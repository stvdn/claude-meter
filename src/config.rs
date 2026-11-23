//! User settings in `%APPDATA%\claude-meter\config.toml`. Nothing secret lives here.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Seconds between usage checks.
    pub interval_secs: u64,
    pub show_badge: bool,
    pub compact: bool,
    /// Badge position in physical pixels; `None` = bottom-right of the main screen.
    pub badge_x: Option<i32>,
    pub badge_y: Option<i32>,
    /// Cached claude.ai organization id, so each poll is one request.
    pub org_id: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_secs: 60,
            show_badge: true,
            compact: false,
            badge_x: None,
            badge_y: None,
            org_id: None,
        }
    }
}

/// `%APPDATA%\claude-meter` (roaming settings).
pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("claude-meter")
}

/// `%LOCALAPPDATA%\claude-meter` (the login window's browser profile).
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("claude-meter")
}

fn path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config_dir());
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = std::fs::write(path(), s);
        }
    }

    /// Never poll faster than every 30 s, whatever the file says.
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_secs.max(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let c: Config = toml::from_str("compact = true").unwrap();
        assert!(c.compact);
        assert_eq!(c.interval_secs, 60);
        assert!(c.show_badge);
    }

    #[test]
    fn round_trips() {
        let c = Config { badge_x: Some(10), badge_y: Some(-5), org_id: Some("abc".into()), ..Config::default() };
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn interval_has_a_floor() {
        let c = Config { interval_secs: 1, ..Config::default() };
        assert_eq!(c.interval().as_secs(), 30);
    }
}
