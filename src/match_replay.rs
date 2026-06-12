use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::input::{self, Player};
use crate::memory::{self, Endian};
use crate::mk2_addrs;
use crate::retro::Core;
use crate::score;

const MAGIC: &[u8; 4] = b"NCRP";
const VERSION: u16 = 1;
const REPLAY_DIR: &str = "replays";
const HIT_MARKER_COOLDOWN_FRAMES: u32 = 18;
const BIG_DAMAGE_THRESHOLD: u16 = 40;
const DAMAGE_SEQUENCE_GAP_FRAMES: u32 = 55;
const LOW_HEALTH_THRESHOLD: u16 = 40;
const BOOKMARK_TOGGLE_TOLERANCE_FRAMES: u32 = 18;
const NOTES_MAGIC: &str = "FREEPLAY_REPLAY_NOTES_V1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayMeta {
    pub filename: String,
    pub path: String,
    pub p1_name: String,
    pub p2_name: String,
    pub p1_score: Option<u16>,
    pub p2_score: Option<u16>,
    pub winner: String,
    pub frame_count: u32,
    pub duration: String,
    pub note: String,
    pub bookmark_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayNotes {
    pub note: String,
    pub bookmarks: Vec<ReplayBookmark>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayBookmark {
    pub frame: u32,
    pub note: String,
}

pub struct Recording {
    initial_state: Option<Vec<u8>>,
    inputs: Vec<[u16; 2]>,
    base_frame: Option<i32>,
    confirmed_frame: Option<i32>,
    p1_name: String,
    p2_name: String,
    started_unix: u64,
}

impl Recording {
    pub fn new(p1_name: impl Into<String>, p2_name: impl Into<String>) -> Self {
        Self {
            initial_state: None,
            inputs: Vec::new(),
            base_frame: None,
            confirmed_frame: None,
            p1_name: clean_name(&p1_name.into()),
            p2_name: clean_name(&p2_name.into()),
            started_unix: unix_now(),
        }
    }

    pub fn record_frame(&mut self, core: &Core, frame: i32, p1_bits: u16, p2_bits: u16) {
        if frame < 0 {
            return;
        }
        if self.initial_state.is_none() {
            self.initial_state = core.save_state();
            self.base_frame = Some(frame);
            if let Some(state) = &self.initial_state {
                println!(
                    "[replay] Anchor state captured at frame {frame} ({} bytes)",
                    state.len()
                );
            }
        }
        let Some(base_frame) = self.base_frame else {
            return;
        };
        if frame < base_frame {
            println!("[replay] Ignoring rollback frame {frame} before replay anchor {base_frame}");
            return;
        }
        if self.initial_state.is_some() {
            let index = (frame - base_frame) as usize;
            if self.inputs.len() <= index {
                self.inputs.resize(index + 1, [0, 0]);
            }
            self.inputs[index] = [p1_bits, p2_bits];
        }
    }

    pub fn set_confirmed_frame(&mut self, frame: i32) {
        if frame < 0 {
            return;
        }
        self.confirmed_frame = Some(self.confirmed_frame.map_or(frame, |prev| prev.max(frame)));
    }

    pub fn frame_count(&self) -> usize {
        self.savable_frame_count()
    }

    pub fn save_default(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(REPLAY_DIR)?;
        let filename = format!(
            "{}_{}_vs_{}.ncrp",
            self.started_unix,
            filename_part(&self.p1_name),
            filename_part(&self.p2_name)
        );
        let path = Path::new(REPLAY_DIR).join(filename);
        self.save(&path)?;
        Ok(path)
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let Some(state) = &self.initial_state else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no anchor state captured",
            ));
        };
        let mut f = std::fs::File::create(path)?;
        let input_count = self.savable_frame_count();
        write_header(
            &mut f,
            input_count as u32,
            state.len() as u32,
            &self.p1_name,
            &self.p2_name,
        )?;
        f.write_all(state)?;
        for frame in self.inputs.iter().take(input_count) {
            f.write_all(&frame[0].to_le_bytes())?;
            f.write_all(&frame[1].to_le_bytes())?;
        }
        Ok(())
    }

    fn savable_frame_count(&self) -> usize {
        let Some(base_frame) = self.base_frame else {
            return 0;
        };
        let confirmed_count = self
            .confirmed_frame
            .map(|frame| {
                if frame < base_frame {
                    0
                } else {
                    (frame - base_frame + 1) as usize
                }
            })
            .unwrap_or(self.inputs.len());
        confirmed_count.min(self.inputs.len())
    }
}

pub struct Playback {
    source_path: PathBuf,
    state: Vec<u8>,
    inputs: Vec<[u16; 2]>,
    cursor: usize,
    p1_name: String,
    p2_name: String,
    notes: ReplayNotes,
    markers: Vec<ReplayMarker>,
    last_score: Option<score::Score>,
    last_hp: Option<(u16, u16)>,
    last_hit_marker_frame: Option<u32>,
    round_first_hit_seen: bool,
    damage_sequence_total: u16,
    damage_sequence_gap: u32,
    damage_sequence_marked: bool,
    low_health_marked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayMarkerKind {
    RoundStart,
    RoundWinP1,
    RoundWinP2,
    Hit,
    FirstHit,
    BigDamage,
    LowHealth,
    MatchEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayEventFilter {
    All,
    Hits,
    Learning,
    Bookmarks,
    Rounds,
    MatchEnd,
}

impl ReplayEventFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Hits,
            Self::Hits => Self::Learning,
            Self::Learning => Self::Bookmarks,
            Self::Bookmarks => Self::Rounds,
            Self::Rounds => Self::MatchEnd,
            Self::MatchEnd => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Hits => "HITS",
            Self::Learning => "LEARN",
            Self::Bookmarks => "BOOKMARKS",
            Self::Rounds => "ROUNDS",
            Self::MatchEnd => "END",
        }
    }

    pub fn matches_marker(self, kind: ReplayMarkerKind) -> bool {
        match self {
            Self::All => true,
            Self::Hits => matches!(
                kind,
                ReplayMarkerKind::Hit | ReplayMarkerKind::FirstHit | ReplayMarkerKind::BigDamage
            ),
            Self::Learning => kind.is_learning_marker(),
            Self::Bookmarks => false,
            Self::Rounds => matches!(
                kind,
                ReplayMarkerKind::RoundStart
                    | ReplayMarkerKind::RoundWinP1
                    | ReplayMarkerKind::RoundWinP2
            ),
            Self::MatchEnd => kind == ReplayMarkerKind::MatchEnd,
        }
    }

    pub fn matches_bookmarks(self) -> bool {
        matches!(self, Self::All | Self::Bookmarks)
    }
}

impl ReplayMarkerKind {
    pub fn is_learning_marker(self) -> bool {
        matches!(
            self,
            Self::RoundWinP1
                | Self::RoundWinP2
                | Self::FirstHit
                | Self::BigDamage
                | Self::LowHealth
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayMarker {
    pub frame: u32,
    pub kind: ReplayMarkerKind,
}

impl Playback {
    pub fn load<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let source_path = path.as_ref().to_path_buf();
        let mut f = std::fs::File::open(&source_path)?;
        let header = read_header(&mut f)?;
        let mut state = vec![0u8; header.state_size as usize];
        f.read_exact(&mut state)?;
        let mut inputs = Vec::with_capacity(header.frame_count as usize);
        for _ in 0..header.frame_count {
            let mut buf = [0u8; 4];
            f.read_exact(&mut buf)?;
            inputs.push([
                u16::from_le_bytes([buf[0], buf[1]]),
                u16::from_le_bytes([buf[2], buf[3]]),
            ]);
        }
        Ok(Self {
            notes: load_replay_notes(&source_path),
            source_path,
            state,
            inputs,
            cursor: 0,
            p1_name: header.p1_name,
            p2_name: header.p2_name,
            markers: Vec::new(),
            last_score: None,
            last_hp: None,
            last_hit_marker_frame: None,
            round_first_hit_seen: false,
            damage_sequence_total: 0,
            damage_sequence_gap: 0,
            damage_sequence_marked: false,
            low_health_marked: false,
        })
    }

    pub fn prime(&self, core: &Core) -> bool {
        let ok = core.load_state(&self.state);
        if ok {
            println!(
                "[replay] Loaded anchor state ({} bytes), {} frames queued",
                self.state.len(),
                self.inputs.len()
            );
        }
        ok
    }

    pub fn reset_to_anchor(&mut self, core: &Core) -> bool {
        if !core.load_state(&self.state) {
            return false;
        }
        self.cursor = 0;
        self.last_score = None;
        self.last_hp = None;
        self.last_hit_marker_frame = None;
        self.reset_round_learning_state();
        input::clear_all_inputs();
        true
    }

    pub fn inject_next(&mut self) -> bool {
        let Some(frame) = self.inputs.get(self.cursor).copied() else {
            return false;
        };
        input::apply_snapshot(Player::P1, frame[0]);
        input::apply_snapshot(Player::P2, frame[1]);
        self.cursor += 1;
        true
    }

    pub fn observe_current_frame(&mut self, core: &Core) {
        let frame = self.cursor.min(u32::MAX as usize) as u32;
        let now_score = score::Score::read(core);
        if let Some(prev) = self.last_score {
            if now_score.round_num > 0 && now_score.round_num != prev.round_num {
                self.push_marker(frame, ReplayMarkerKind::RoundStart);
                self.reset_round_learning_state();
            }
            if now_score.p1_match_wins == prev.p1_match_wins + 1 {
                self.push_marker(frame, ReplayMarkerKind::RoundWinP1);
            }
            if now_score.p2_match_wins == prev.p2_match_wins + 1 {
                self.push_marker(frame, ReplayMarkerKind::RoundWinP2);
            }
            let was_decided = prev.p1_match_wins >= 2 || prev.p2_match_wins >= 2;
            let is_decided = now_score.p1_match_wins >= 2 || now_score.p2_match_wins >= 2;
            if !was_decided && is_decided {
                self.push_marker(frame, ReplayMarkerKind::MatchEnd);
            }
        } else if now_score.round_num > 0 {
            self.push_marker(frame, ReplayMarkerKind::RoundStart);
            self.reset_round_learning_state();
        }
        self.last_score = Some(now_score);

        let hp = (
            memory::peek_u16(core, mk2_addrs::P1_HP_ADDR, Endian::Little).unwrap_or(0),
            memory::peek_u16(core, mk2_addrs::P2_HP_ADDR, Endian::Little).unwrap_or(0),
        );
        if let Some(prev) = self.last_hp {
            let p1_damage = damage_taken(prev.0, hp.0);
            let p2_damage = damage_taken(prev.1, hp.1);
            let damage = p1_damage.saturating_add(p2_damage);
            let took_damage = damage > 0;
            let cooled_down = self
                .last_hit_marker_frame
                .map(|last| frame.saturating_sub(last) >= HIT_MARKER_COOLDOWN_FRAMES)
                .unwrap_or(true);
            if took_damage && !self.round_first_hit_seen {
                self.push_marker(frame, ReplayMarkerKind::FirstHit);
                self.round_first_hit_seen = true;
            }
            if took_damage && cooled_down {
                self.push_marker(frame, ReplayMarkerKind::Hit);
                self.last_hit_marker_frame = Some(frame);
            }
            self.observe_damage_sequence(frame, damage);
            if !self.low_health_marked
                && hp.0 > 0
                && hp.1 > 0
                && hp.0 <= LOW_HEALTH_THRESHOLD
                && hp.1 <= LOW_HEALTH_THRESHOLD
            {
                self.push_marker(frame, ReplayMarkerKind::LowHealth);
                self.low_health_marked = true;
            }
        } else if hp.0 == 0 || hp.1 == 0 {
            self.reset_round_learning_state();
        }
        self.last_hp = Some(hp);
    }

    fn observe_damage_sequence(&mut self, frame: u32, damage: u16) {
        if damage > 0 {
            if self.damage_sequence_gap == 0 {
                self.damage_sequence_total = 0;
                self.damage_sequence_marked = false;
            }
            self.damage_sequence_total = self.damage_sequence_total.saturating_add(damage);
            self.damage_sequence_gap = DAMAGE_SEQUENCE_GAP_FRAMES;
            if !self.damage_sequence_marked && self.damage_sequence_total >= BIG_DAMAGE_THRESHOLD {
                self.push_marker(frame, ReplayMarkerKind::BigDamage);
                self.damage_sequence_marked = true;
            }
        } else if self.damage_sequence_gap > 0 {
            self.damage_sequence_gap -= 1;
            if self.damage_sequence_gap == 0 {
                self.damage_sequence_total = 0;
                self.damage_sequence_marked = false;
            }
        }
    }

    fn reset_round_learning_state(&mut self) {
        self.round_first_hit_seen = false;
        self.damage_sequence_total = 0;
        self.damage_sequence_gap = 0;
        self.damage_sequence_marked = false;
        self.low_health_marked = false;
    }

    fn push_marker(&mut self, frame: u32, kind: ReplayMarkerKind) {
        if self
            .markers
            .iter()
            .any(|m| m.frame == frame && m.kind == kind)
        {
            return;
        }
        self.markers.push(ReplayMarker { frame, kind });
        self.markers.sort_by_key(|marker| marker.frame);
    }

    pub fn toggle_bookmark_at_current(&mut self) -> std::io::Result<bool> {
        let frame = self.current_frame().min(u32::MAX as usize) as u32;
        if let Some(index) = self.nearest_bookmark_index(frame, BOOKMARK_TOGGLE_TOLERANCE_FRAMES) {
            self.notes.bookmarks.remove(index);
            self.save_notes()?;
            return Ok(false);
        }
        self.notes.bookmarks.push(ReplayBookmark {
            frame,
            note: String::new(),
        });
        self.notes.bookmarks.sort_by_key(|bookmark| bookmark.frame);
        self.save_notes()?;
        Ok(true)
    }

    pub fn remove_bookmark_near_current(
        &mut self,
        tolerance: u32,
    ) -> std::io::Result<Option<ReplayBookmark>> {
        let frame = self.current_frame().min(u32::MAX as usize) as u32;
        let Some(index) = self.nearest_bookmark_index(frame, tolerance) else {
            return Ok(None);
        };
        let removed = self.notes.bookmarks.remove(index);
        self.save_notes()?;
        Ok(Some(removed))
    }

    fn nearest_bookmark_index(&self, frame: u32, tolerance: u32) -> Option<usize> {
        self.notes
            .bookmarks
            .iter()
            .enumerate()
            .filter_map(|(index, bookmark)| {
                let distance = bookmark.frame.abs_diff(frame);
                (distance <= tolerance).then_some((index, distance))
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(index, _)| index)
    }

    fn save_notes(&self) -> std::io::Result<()> {
        save_replay_notes(&self.source_path, &self.notes)
    }

    pub fn current_frame(&self) -> usize {
        self.cursor
    }

    pub fn current_inputs(&self) -> Option<[u16; 2]> {
        if self.cursor == 0 {
            self.inputs.first().copied()
        } else {
            self.inputs.get(self.cursor - 1).copied()
        }
    }

    pub fn markers(&self) -> &[ReplayMarker] {
        &self.markers
    }

    pub fn bookmarks(&self) -> &[ReplayBookmark] {
        &self.notes.bookmarks
    }

    pub fn next_bookmark_after(&self, frame: usize) -> Option<ReplayBookmark> {
        self.notes
            .bookmarks
            .iter()
            .find(|bookmark| bookmark.frame as usize > frame)
            .cloned()
    }

    pub fn next_event_frame_after(&self, frame: usize, filter: ReplayEventFilter) -> Option<u32> {
        self.markers
            .iter()
            .filter(|marker| marker.frame as usize > frame && filter.matches_marker(marker.kind))
            .map(|marker| marker.frame)
            .chain(
                self.notes
                    .bookmarks
                    .iter()
                    .filter(|bookmark| {
                        bookmark.frame as usize > frame && filter.matches_bookmarks()
                    })
                    .map(|bookmark| bookmark.frame),
            )
            .min()
    }

    pub fn previous_event_frame_before(
        &self,
        frame: usize,
        filter: ReplayEventFilter,
    ) -> Option<u32> {
        self.markers
            .iter()
            .filter(|marker| (marker.frame as usize) < frame && filter.matches_marker(marker.kind))
            .map(|marker| marker.frame)
            .chain(
                self.notes
                    .bookmarks
                    .iter()
                    .filter(|bookmark| {
                        (bookmark.frame as usize) < frame && filter.matches_bookmarks()
                    })
                    .map(|bookmark| bookmark.frame),
            )
            .max()
    }

    pub fn frame_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn p1_name(&self) -> &str {
        &self.p1_name
    }

    pub fn p2_name(&self) -> &str {
        &self.p2_name
    }
}

pub fn list_replays() -> Vec<ReplayMeta> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(REPLAY_DIR) else {
        return out;
    };
    for entry in dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ncrp") {
            continue;
        }
        if let Ok(meta) = read_meta(&path) {
            out.push(meta);
        }
    }
    out.sort_by(|a, b| b.filename.cmp(&a.filename));
    out
}

pub fn list_online_replays() -> Vec<ReplayMeta> {
    list_replays()
        .into_iter()
        .filter(looks_like_online_replay)
        .collect()
}

pub fn replay_notes_path<P: AsRef<Path>>(path: P) -> PathBuf {
    path.as_ref().with_extension("ncrp.notes")
}

pub fn save_replay_note<P: AsRef<Path>>(path: P, note: &str) -> std::io::Result<()> {
    let mut notes = load_replay_notes(path.as_ref());
    notes.note = clean_note_text(note, 96);
    save_replay_notes(path, &notes)
}

pub fn finalize_recording(rec: &mut Option<Recording>) -> Option<PathBuf> {
    let rec = rec.take()?;
    if rec.frame_count() == 0 {
        println!("[replay] Session produced 0 frames, nothing to save.");
        return None;
    }
    match rec.save_default() {
        Ok(path) => {
            println!(
                "[replay] Saved {} frames to {}",
                rec.frame_count(),
                path.display()
            );
            Some(path)
        }
        Err(e) => {
            println!("[replay] Save failed: {e}");
            None
        }
    }
}

fn read_meta(path: &Path) -> std::io::Result<ReplayMeta> {
    let mut f = std::fs::File::open(path)?;
    let header = read_header(&mut f)?;
    let notes = load_replay_notes(path);
    let summary = load_replay_summary(path);
    Ok(ReplayMeta {
        filename: path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown.ncrp")
            .to_string(),
        path: path.to_string_lossy().into_owned(),
        p1_name: header.p1_name,
        p2_name: header.p2_name,
        p1_score: summary.p1_score,
        p2_score: summary.p2_score,
        winner: summary.winner,
        frame_count: header.frame_count,
        duration: summary.duration,
        note: notes.note,
        bookmark_count: notes.bookmarks.len(),
    })
}

#[derive(Default)]
struct ReplaySummary {
    p1_score: Option<u16>,
    p2_score: Option<u16>,
    winner: String,
    duration: String,
}

fn load_replay_summary(path: &Path) -> ReplaySummary {
    let Ok(text) = std::fs::read_to_string(path.with_extension("ncrp.json")) else {
        return ReplaySummary::default();
    };
    ReplaySummary {
        p1_score: summary_json_u64(&text, "p1_score").map(|v| v.min(u16::MAX as u64) as u16),
        p2_score: summary_json_u64(&text, "p2_score").map(|v| v.min(u16::MAX as u64) as u16),
        winner: summary_json_str(&text, "winner").unwrap_or_default(),
        duration: summary_json_str(&text, "duration").unwrap_or_default(),
    }
}

fn summary_json_str(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let start = json.find(&pat)? + pat.len();
    let end = json[start..].find('"')?;
    Some(json[start..start + end].to_string())
}

fn summary_json_u64(json: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\": ");
    let start = json.find(&pat)? + pat.len();
    let tail = &json[start..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    tail[..end].parse().ok()
}

fn load_replay_notes<P: AsRef<Path>>(path: P) -> ReplayNotes {
    let notes_path = replay_notes_path(path);
    let Ok(text) = std::fs::read_to_string(notes_path) else {
        return ReplayNotes::default();
    };
    parse_replay_notes(&text)
}

fn save_replay_notes<P: AsRef<Path>>(path: P, notes: &ReplayNotes) -> std::io::Result<()> {
    let notes_path = replay_notes_path(path);
    if notes.note.is_empty() && notes.bookmarks.is_empty() {
        match std::fs::remove_file(notes_path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
    }

    if let Some(parent) = notes_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    out.push_str(NOTES_MAGIC);
    out.push('\n');
    if !notes.note.is_empty() {
        out.push_str("note\t");
        out.push_str(&clean_note_text(&notes.note, 96));
        out.push('\n');
    }
    for bookmark in &notes.bookmarks {
        out.push_str("bookmark\t");
        out.push_str(&bookmark.frame.to_string());
        out.push('\t');
        out.push_str(&clean_note_text(&bookmark.note, 64));
        out.push('\n');
    }
    std::fs::write(notes_path, out)
}

fn parse_replay_notes(text: &str) -> ReplayNotes {
    let mut notes = ReplayNotes::default();
    for line in text.lines() {
        if line == NOTES_MAGIC || line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("note\t") {
            notes.note = clean_note_text(rest, 96);
        } else if let Some(rest) = line.strip_prefix("bookmark\t") {
            let mut parts = rest.splitn(2, '\t');
            let frame = parts.next().and_then(|raw| raw.parse::<u32>().ok());
            if let Some(frame) = frame {
                notes.bookmarks.push(ReplayBookmark {
                    frame,
                    note: clean_note_text(parts.next().unwrap_or(""), 64),
                });
            }
        }
    }
    notes.bookmarks.sort_by_key(|bookmark| bookmark.frame);
    notes.bookmarks.dedup_by_key(|bookmark| bookmark.frame);
    notes
}

fn clean_note_text(note: &str, max_chars: usize) -> String {
    note.trim()
        .chars()
        .filter_map(|c| {
            if c == '\t' {
                Some(' ')
            } else if c.is_control() {
                None
            } else {
                Some(c)
            }
        })
        .take(max_chars)
        .collect()
}

fn looks_like_online_replay(meta: &ReplayMeta) -> bool {
    !is_local_replay_name(&meta.p1_name) && !is_local_replay_name(&meta.p2_name)
}

fn is_local_replay_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "cpu" | "lab" | "ghost" | "drone"
    )
}

fn damage_taken(previous_hp: u16, current_hp: u16) -> u16 {
    if previous_hp > 0 && current_hp > 0 && current_hp < previous_hp {
        previous_hp - current_hp
    } else {
        0
    }
}

struct Header {
    frame_count: u32,
    state_size: u32,
    p1_name: String,
    p2_name: String,
}

fn write_header<W: Write>(
    w: &mut W,
    frame_count: u32,
    state_size: u32,
    p1_name: &str,
    p2_name: &str,
) -> std::io::Result<()> {
    let p1 = p1_name.as_bytes();
    let p2 = p2_name.as_bytes();
    if p1.len() > u16::MAX as usize || p2.len() > u16::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "player name too long",
        ));
    }
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&frame_count.to_le_bytes())?;
    w.write_all(&state_size.to_le_bytes())?;
    w.write_all(&(p1.len() as u16).to_le_bytes())?;
    w.write_all(&(p2.len() as u16).to_le_bytes())?;
    w.write_all(p1)?;
    w.write_all(p2)?;
    Ok(())
}

fn read_header<R: Read>(r: &mut R) -> std::io::Result<Header> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("bad replay magic: expected NCRP, got {:?}", magic),
        ));
    }
    let version = read_u16(r)?;
    if version != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported replay version: {version}"),
        ));
    }
    let frame_count = read_u32(r)?;
    let state_size = read_u32(r)?;
    let p1_len = read_u16(r)? as usize;
    let p2_len = read_u16(r)? as usize;
    let mut p1 = vec![0u8; p1_len];
    let mut p2 = vec![0u8; p2_len];
    r.read_exact(&mut p1)?;
    r.read_exact(&mut p2)?;
    Ok(Header {
        frame_count,
        state_size,
        p1_name: String::from_utf8_lossy(&p1).into_owned(),
        p2_name: String::from_utf8_lossy(&p2).into_owned(),
    })
}

fn read_u16<R: Read>(r: &mut R) -> std::io::Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn clean_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "Player".into()
    } else {
        trimmed.chars().take(32).collect()
    }
}

fn filename_part(name: &str) -> String {
    let mut out: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    if out.is_empty() {
        out = "Player".into();
    }
    out
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_parts_are_safe() {
        assert_eq!(filename_part("Liu Kang!"), "LiuKang");
        assert_eq!(filename_part(""), "Player");
    }

    #[test]
    fn header_roundtrips_metadata() {
        let mut bytes = Vec::new();
        write_header(&mut bytes, 123, 456, "P1", "Opponent").unwrap();
        let header = read_header(&mut bytes.as_slice()).unwrap();
        assert_eq!(header.frame_count, 123);
        assert_eq!(header.state_size, 456);
        assert_eq!(header.p1_name, "P1");
        assert_eq!(header.p2_name, "Opponent");
    }

    #[test]
    fn recording_frame_count_tracks_confirmed_frames() {
        let mut rec = Recording::new("P1", "P2");
        rec.initial_state = Some(vec![1, 2, 3]);
        rec.base_frame = Some(10);
        rec.inputs = vec![[0x0001, 0x0002], [0x0003, 0x0004], [0x0005, 0x0006]];

        assert_eq!(rec.frame_count(), 3);
        rec.set_confirmed_frame(11);
        assert_eq!(rec.frame_count(), 2);
        rec.set_confirmed_frame(12);
        assert_eq!(rec.frame_count(), 3);
    }

    #[test]
    fn replay_notes_parse_bookmarks() {
        let notes = parse_replay_notes(
            "FREEPLAY_REPLAY_NOTES_V1\nnote\tAnti-air more\nbookmark\t120\tjump punish\nbookmark\t30\t\n",
        );
        assert_eq!(notes.note, "Anti-air more");
        assert_eq!(notes.bookmarks.len(), 2);
        assert_eq!(notes.bookmarks[0].frame, 30);
        assert_eq!(notes.bookmarks[1].frame, 120);
        assert_eq!(notes.bookmarks[1].note, "jump punish");
    }

    #[test]
    fn online_replay_filter_excludes_local_training_names() {
        let online = ReplayMeta {
            filename: "online.ncrp".into(),
            path: "online.ncrp".into(),
            p1_name: "PlayerOne".into(),
            p2_name: "Opponent".into(),
            p1_score: None,
            p2_score: None,
            winner: String::new(),
            frame_count: 10,
            duration: String::new(),
            note: String::new(),
            bookmark_count: 0,
        };
        let arcade = ReplayMeta {
            p2_name: "CPU".into(),
            ..online.clone()
        };
        let lab = ReplayMeta {
            p2_name: "Lab".into(),
            ..online.clone()
        };
        let ghost = ReplayMeta {
            p2_name: "Ghost".into(),
            ..online.clone()
        };

        assert!(looks_like_online_replay(&online));
        assert!(!looks_like_online_replay(&arcade));
        assert!(!looks_like_online_replay(&lab));
        assert!(!looks_like_online_replay(&ghost));
    }

    #[test]
    fn replay_event_filter_matches_expected_events() {
        assert!(ReplayEventFilter::All.matches_marker(ReplayMarkerKind::Hit));
        assert!(ReplayEventFilter::All.matches_bookmarks());
        assert!(ReplayEventFilter::Hits.matches_marker(ReplayMarkerKind::Hit));
        assert!(ReplayEventFilter::Hits.matches_marker(ReplayMarkerKind::BigDamage));
        assert!(ReplayEventFilter::Learning.matches_marker(ReplayMarkerKind::FirstHit));
        assert!(ReplayEventFilter::Learning.matches_marker(ReplayMarkerKind::LowHealth));
        assert!(!ReplayEventFilter::Hits.matches_marker(ReplayMarkerKind::RoundStart));
        assert!(!ReplayEventFilter::Learning.matches_marker(ReplayMarkerKind::Hit));
        assert!(ReplayEventFilter::Rounds.matches_marker(ReplayMarkerKind::RoundWinP1));
        assert!(ReplayEventFilter::Bookmarks.matches_bookmarks());
        assert!(!ReplayEventFilter::Bookmarks.matches_marker(ReplayMarkerKind::Hit));
        assert_eq!(ReplayEventFilter::All.next(), ReplayEventFilter::Hits);
        assert_eq!(ReplayEventFilter::Hits.next(), ReplayEventFilter::Learning);
        assert_eq!(ReplayEventFilter::MatchEnd.next(), ReplayEventFilter::All);
    }

    #[test]
    fn damage_taken_ignores_zeroed_vs_screen_health() {
        assert_eq!(damage_taken(0, 120), 0);
        assert_eq!(damage_taken(120, 0), 0);
        assert_eq!(damage_taken(120, 100), 20);
    }
}
