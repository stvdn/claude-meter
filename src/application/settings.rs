//! User settings as data. Where they're stored is the `SettingsStore` port's business.
//! Nothing secret lives here.

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Seconds between usage checks.
    pub interval_secs: u64,
    pub show_badge: bool,
    pub compact: bool,
    /// Badge position in physical pixels; `None` = bottom-right of the main screen.
    pub badge_x: Option<i32>,
    pub badge_y: Option<i32>,
    /// Cached organization id, so each poll is one request.
    pub org_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { interval_secs: 60, show_badge: true, compact: false, badge_x: None, badge_y: None, org_id: None }
    }
}

impl Settings {
    /// Never poll faster than every 30 s, whatever the file says.
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_secs.max(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_has_a_floor() {
        let s = Settings { interval_secs: 1, ..Settings::default() };
        assert_eq!(s.interval().as_secs(), 30);
        assert_eq!(Settings::default().interval().as_secs(), 60);
    }
}
