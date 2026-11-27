//! What usage *is* and how urgent it is. Pure data and rules: no I/O, no text for
//! display, no knowledge of claude.ai's JSON or of Windows.

use chrono::{DateTime, Utc};

/// One rate-limit window (the 5-hour session, the 7-day week, or a per-model week).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Window {
    /// Percent of the window already used, usually 0..=100.
    pub percent: f32,
    /// When the window resets. `None` when no usage has started it yet.
    pub resets_at: Option<DateTime<Utc>>,
    /// The provider's own verdict for this window, when it gave one.
    pub severity: Option<Level>,
}

impl Window {
    /// The worse of our thresholds and the provider's severity, so the server can only raise it.
    pub fn level(&self) -> Level {
        level(self.percent).max(self.severity.unwrap_or(Level::Ok))
    }
}

/// A limit beyond the session and the overall week, e.g. a weekly cap on one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// The provider's id for it, e.g. `weekly_opus`.
    pub kind: String,
    pub window: Window,
}

/// One product's share of this week's usage, e.g. Claude Code 88%.
#[derive(Debug, Clone, PartialEq)]
pub struct Share {
    pub name: String,
    pub percent: u8,
}

/// Everything the meter knows. A window is `None` when the server didn't report it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    /// Other limits, in the provider's order.
    pub others: Vec<Limit>,
    /// Who used this week's allowance, largest first; zero shares are left out.
    pub breakdown: Vec<Share>,
}

impl Usage {
    /// The most urgent level across every window.
    pub fn level(&self) -> Level {
        self.session
            .iter()
            .chain(&self.weekly)
            .chain(self.others.iter().map(|l| &l.window))
            .map(Window::level)
            .max()
            .unwrap_or(Level::Ok)
    }
}

/// Traffic-light level. Ordered, so `max` picks the most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Critical,
}

pub const WARN_AT: f32 = 60.0;
pub const CRITICAL_AT: f32 = 85.0;

pub fn level(percent: f32) -> Level {
    if percent >= CRITICAL_AT {
        Level::Critical
    } else if percent >= WARN_AT {
        Level::Warn
    } else {
        Level::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_thresholds() {
        assert_eq!(level(0.0), Level::Ok);
        assert_eq!(level(59.9), Level::Ok);
        assert_eq!(level(60.0), Level::Warn);
        assert_eq!(level(84.9), Level::Warn);
        assert_eq!(level(85.0), Level::Critical);
        assert_eq!(level(130.0), Level::Critical);
    }

    #[test]
    fn server_severity_can_only_raise_the_level() {
        let w = |percent, severity| Window { percent, resets_at: None, severity };
        assert_eq!(w(10.0, Some(Level::Critical)).level(), Level::Critical);
        assert_eq!(w(90.0, Some(Level::Ok)).level(), Level::Critical);
        assert_eq!(w(70.0, None).level(), Level::Warn);
    }

    #[test]
    fn usage_level_is_the_worst_window() {
        let mut u = Usage {
            session: Some(Window { percent: 10.0, ..Window::default() }),
            weekly: Some(Window { percent: 65.0, ..Window::default() }),
            ..Usage::default()
        };
        assert_eq!(u.level(), Level::Warn);
        u.others.push(Limit { kind: "weekly_opus".into(), window: Window { percent: 95.0, ..Window::default() } });
        assert_eq!(u.level(), Level::Critical);
        assert_eq!(Usage::default().level(), Level::Ok);
    }
}
