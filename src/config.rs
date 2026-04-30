//! Persisted configuration: bindings for now; room to grow later.
use crate::input::Bindings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const CONFIG_PATH: &str = "config.toml";

static SIGNALING_URL: Mutex<Option<String>> = Mutex::new(None);

pub fn set_signaling_url(url: String) {
    let trimmed = url.trim_end_matches('/').to_string();
    if let Ok(mut guard) = SIGNALING_URL.lock() {
        *guard = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
    }
}

pub fn signaling_url() -> Option<String> {
    if let Ok(guard) = SIGNALING_URL.lock() {
        if let Some(ref url) = *guard {
            if !url.is_empty() {
                return Some(url.clone());
            }
        }
    }
    None
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub bindings: Bindings,
    /// Public signaling/matchmaking service URL.
    #[serde(default)]
    pub signaling_url: String,
    /// Discord Rich Presence application ID.
    #[serde(default)]
    pub discord_client_id: String,
    /// Optional Discord webhook URL. When non-empty, Freeplay posts round/match
    /// results here during netplay sessions. Get one from any Discord channel
    /// via Edit Channel → Integrations → Webhooks → New Webhook → Copy URL.
    #[serde(default)]
    pub discord_webhook_url: String,
    /// Optional stats service URL for ghost uploads and leaderboards.
    #[serde(default)]
    pub stats_url: String,
}

pub fn path() -> PathBuf {
    Path::new(CONFIG_PATH).to_path_buf()
}

pub fn load() -> Config {
    load_dotenv();
    let mut cfg = match std::fs::read_to_string(path()) {
        Ok(s) => match toml::from_str(&s) {
            Ok(cfg) => {
                println!("Loaded config from {}", path().display());
                cfg
            }
            Err(e) => {
                println!("Config parse error ({e}); using defaults");
                Config::default()
            }
        },
        Err(_) => {
            println!("No config at {} — using defaults", path().display());
            Config::default()
        }
    };
    apply_env_overrides(&mut cfg);
    cfg
}

pub fn save(cfg: &Config) {
    match toml::to_string_pretty(cfg) {
        Ok(s) => {
            if let Err(e) = std::fs::write(path(), s) {
                println!("Failed to write config: {e}");
            } else {
                println!("Saved config to {}", path().display());
            }
        }
        Err(e) => println!("Failed to serialize config: {e}"),
    }
}

pub fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| dotenv_value(key))
}

fn apply_env_overrides(cfg: &mut Config) {
    if let Some(v) = env_value("FREEPLAY_SIGNALING_URL") {
        cfg.signaling_url = v;
    }
    if let Some(v) = env_value("FREEPLAY_DISCORD_CLIENT_ID") {
        cfg.discord_client_id = v;
    }
    if let Some(v) = env_value("FREEPLAY_DISCORD_WEBHOOK_URL") {
        cfg.discord_webhook_url = v;
    }
    if let Some(v) = env_value("FREEPLAY_STATS_URL") {
        cfg.stats_url = v;
    }
}

fn load_dotenv() {
    let Some(path) = find_dotenv() else {
        return;
    };
    let Ok(s) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in s.lines().filter_map(parse_env_line) {
        if std::env::var_os(&line.0).is_none() {
            std::env::set_var(line.0, line.1);
        }
    }
}

fn dotenv_value(key: &str) -> Option<String> {
    let path = find_dotenv()?;
    let s = std::fs::read_to_string(&path).ok()?;
    for (k, v) in s.lines().filter_map(parse_env_line) {
        if k == key && !v.trim().is_empty() {
            return Some(v);
        }
    }
    None
}

fn find_dotenv() -> Option<PathBuf> {
    let cwd = PathBuf::from(".env");
    if cwd.exists() {
        return Some(cwd);
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let beside_exe = exe_dir.join(".env");
    if beside_exe.exists() {
        return Some(beside_exe);
    }
    None
}

fn parse_env_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    Some((key.to_string(), value))
}
