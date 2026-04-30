//! Persisted configuration: bindings for now; room to grow later.
use crate::input::Bindings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CONFIG_PATH: &str = "config.toml";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub bindings: Bindings,
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
    if let Some(v) = env_value("FREEPLAY_DISCORD_WEBHOOK_URL") {
        cfg.discord_webhook_url = v;
    }
    if let Some(v) = env_value("FREEPLAY_STATS_URL") {
        cfg.stats_url = v;
    }
}

fn load_dotenv() {
    let Ok(s) = std::fs::read_to_string(".env") else {
        return;
    };
    for line in s.lines().filter_map(parse_env_line) {
        if std::env::var_os(&line.0).is_none() {
            std::env::set_var(line.0, line.1);
        }
    }
}

fn dotenv_value(key: &str) -> Option<String> {
    let s = std::fs::read_to_string(".env").ok()?;
    for (k, v) in s.lines().filter_map(parse_env_line) {
        if k == key && !v.trim().is_empty() {
            return Some(v);
        }
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
