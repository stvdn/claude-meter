//! The `SettingsStore` adapter: `%APPDATA%\claude-meter\config.toml`.
//! Also knows where the app keeps its local data (the sign-in window's browser profile).

use crate::application::ports::SettingsStore;
use crate::application::settings::Settings;
use std::path::PathBuf;

/// `%APPDATA%\claude-meter` (roaming settings).
pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("claude-meter")
}

/// `%LOCALAPPDATA%\claude-meter` (machine-local data).
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("claude-meter")
}

pub struct SettingsFile {
    path: PathBuf,
}

impl SettingsFile {
    /// The standard location, `%APPDATA%\claude-meter\config.toml`.
    pub fn standard() -> Self {
        Self { path: config_dir().join("config.toml") }
    }
}

/// A missing or unreadable file means defaults, so a broken file never stops the app.
fn parse(text: &str) -> Settings {
    toml::from_str(text).unwrap_or_default()
}

impl SettingsStore for SettingsFile {
    fn load(&self) -> Settings {
        std::fs::read_to_string(&self.path).map(|s| parse(&s)).unwrap_or_default()
    }

    fn save(&self, settings: &Settings) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = toml::to_string_pretty(settings) {
            let _ = std::fs::write(&self.path, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let s = parse("compact = true");
        assert!(s.compact);
        assert_eq!(s.interval_secs, 60);
        assert!(s.show_badge);
        assert_eq!(parse("not toml at all ["), Settings::default());
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("claude-meter-test-{}", std::process::id()));
        let store = SettingsFile { path: dir.join("config.toml") };
        let s = Settings { badge_x: Some(10), badge_y: Some(-5), org_id: Some("abc".into()), ..Settings::default() };
        store.save(&s);
        assert_eq!(store.load(), s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
