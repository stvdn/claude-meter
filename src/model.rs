//! Pure data types and formatting rules.
//!
//! Nothing in this module does I/O or touches Windows APIs, so all of it is
//! covered by plain unit tests (`cargo test`) on any OS.

use chrono::{DateTime, TimeZone, Utc};
use std::fmt::Display;
use std::time::Duration;

/// One rate-limit window (the 5-hour session, the 7-day week, or a per-model week).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Window {
    /// Percent of the window already used, usually 0..=100.
    pub percent: f32,
    /// When the window resets. `None` when no usage has started it yet.
    pub resets_at: Option<DateTime<Utc>>,
    /// claude.ai's own verdict for this window (its `limits[].severity`), when it gave one.
    pub severity: Option<Level>,
}

impl Window {
    /// The worse of our thresholds and claude.ai's severity, so the server can only raise it.
    pub fn level(&self) -> Level {
        level(self.percent).max(self.severity.unwrap_or(Level::Ok))
    }
}

/// A limit beyond the session and the overall week, e.g. a weekly cap on one model.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// claude.ai's id for it, e.g. `weekly_opus`.
    pub kind: String,
    pub window: Window,
}

impl Limit {
    pub fn label(&self) -> String {
        limit_label(&self.kind)
    }
}

/// One product's share of this week's usage (`seven_day_breakdown`), e.g. Claude Code 88%.
#[derive(Debug, Clone, PartialEq)]
pub struct Share {
    pub name: String,
    pub percent: u8,
}

/// Everything the meter shows. A window is `None` when the server didn't report it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    /// Other limits from claude.ai's `limits` list, in its order.
    pub others: Vec<Limit>,
    /// Who used this week's allowance, largest first; zero shares are left out.
    pub breakdown: Vec<Share>,
}

impl Usage {
    /// The most urgent level across every window, for the tray icon's color.
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

/// Traffic-light level used for colors. Ordered, so `max` picks the most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Critical,
}

/// Map claude.ai's `severity` strings. Unknown words give `None`, so our thresholds decide.
pub fn parse_severity(s: &str) -> Option<Level> {
    match s.to_ascii_lowercase().as_str() {
        "normal" | "ok" | "low" => Some(Level::Ok),
        "warning" | "warn" | "elevated" | "approaching" | "medium" => Some(Level::Warn),
        "critical" | "high" | "exceeded" | "limited" | "blocked" | "error" => Some(Level::Critical),
        _ => None,
    }
}

/// `weekly_opus` -> `"Week · Opus"`, `monthly_api` -> `"Monthly Api"`.
pub fn limit_label(kind: &str) -> String {
    let title = |s: &str| {
        s.split('_')
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    match kind {
        "session" => "Session".into(),
        "weekly_all" => "Week".into(),
        k => match k.strip_prefix("weekly_") {
            Some(rest) => format!("Week · {}", title(rest)),
            None => title(k),
        },
    }
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

/// `42.4` -> `"42%"`. Clamped at 0 so a stray negative never shows.
pub fn percent_label(percent: f32) -> String {
    format!("{}%", percent.max(0.0).round() as i64)
}

/// Time left until `resets_at`, rounded up to the minute: `"2h 14m"`, `"38m"`, `"3d 4h"`.
pub fn countdown(now: DateTime<Utc>, resets_at: DateTime<Utc>) -> String {
    let secs = (resets_at - now).num_seconds();
    if secs <= 0 {
        return "now".to_string();
    }
    let mins = (secs + 59) / 60;
    let (days, hours, minutes) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Weekday and clock time in `tz`: `"Mon 9:00 AM"`. Generic so tests can pin a timezone.
pub fn weekday_time<Tz: TimeZone>(t: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    t.with_timezone(tz).format("%a %-I:%M %p").to_string()
}

/// Second line under a bar: "Resets in 2h 14m" for the session, "Resets Mon 9:00 AM" for the week.
pub fn reset_line<Tz: TimeZone>(
    window: &Window,
    now: DateTime<Utc>,
    tz: &Tz,
    weekly: bool,
) -> String
where
    Tz::Offset: Display,
{
    match window.resets_at {
        None => "Not started".to_string(),
        Some(t) if weekly && (t - now).num_hours() >= 24 => format!("Resets {}", weekday_time(t, tz)),
        Some(t) => format!("Resets in {}", countdown(now, t)),
    }
}

/// Tray tooltip text (Windows caps tooltips at 127 characters, this stays well under).
pub fn tooltip<Tz: TimeZone>(usage: &Usage, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let mut lines = Vec::new();
    if let Some(w) = &usage.session {
        lines.push(format!("Session {} · {}", percent_label(w.percent), reset_line(w, now, tz, false)));
    }
    if let Some(w) = &usage.weekly {
        lines.push(format!("Week {} · {}", percent_label(w.percent), reset_line(w, now, tz, true)));
    }
    // Extra limits only while they fit: Windows cuts tooltips at 127 characters.
    for l in &usage.others {
        let line = format!("{} {}", l.label(), percent_label(l.window.percent));
        let used: usize = lines.iter().map(|s| s.chars().count() + 1).sum();
        if used + line.chars().count() < 127 {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        lines.push("No usage data".to_string());
    }
    lines.join("\n")
}

/// Menu line for one window: `"Session 42% · Resets in 2h 14m"`.
pub fn window_line<Tz: TimeZone>(label: &str, w: &Window, now: DateTime<Utc>, tz: &Tz, weekly: bool) -> String
where
    Tz::Offset: Display,
{
    format!("{label} {} · {}", percent_label(w.percent), reset_line(w, now, tz, weekly))
}

/// `"This week: Claude Code 88% · Cowork 12%"`, or `None` when there's nothing to show.
pub fn breakdown_line(shares: &[Share]) -> Option<String> {
    if shares.is_empty() {
        return None;
    }
    let parts: Vec<String> = shares.iter().map(|s| format!("{} {}%", s.name, s.percent)).collect();
    Some(format!("This week: {}", parts.join(" · ")))
}

/// Wait before retrying after `failures` consecutive errors: 30 s, 60 s, 120 s ... capped at 10 min.
pub fn backoff(failures: u32) -> Duration {
    let secs = 30u64.saturating_mul(1u64 << failures.saturating_sub(1).min(10));
    Duration::from_secs(secs.min(600))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

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
    fn percent_rounds_and_clamps() {
        assert_eq!(percent_label(42.4), "42%");
        assert_eq!(percent_label(42.5), "43%");
        assert_eq!(percent_label(-3.0), "0%");
    }

    #[test]
    fn countdown_formats() {
        let now = t("2026-11-23T10:00:00Z");
        assert_eq!(countdown(now, t("2026-11-23T12:14:00Z")), "2h 14m");
        assert_eq!(countdown(now, t("2026-11-23T10:37:01Z")), "38m"); // rounds up
        assert_eq!(countdown(now, t("2026-11-26T14:00:00Z")), "3d 4h");
        assert_eq!(countdown(now, t("2026-11-23T09:00:00Z")), "now");
    }

    #[test]
    fn reset_lines() {
        let now = t("2026-11-23T10:00:00Z"); // a Monday
        let utc = FixedOffset::east_opt(0).unwrap();
        let session = Window { percent: 42.0, resets_at: Some(t("2026-11-23T12:14:00Z")), severity: None };
        let week = Window { percent: 18.0, resets_at: Some(t("2026-11-30T09:00:00Z")), severity: None };
        let fresh = Window::default();
        assert_eq!(reset_line(&session, now, &utc, false), "Resets in 2h 14m");
        assert_eq!(reset_line(&week, now, &utc, true), "Resets Mon 9:00 AM");
        assert_eq!(reset_line(&fresh, now, &utc, false), "Not started");
        // A weekly window ending within a day shows a countdown instead of a weekday.
        let soon = Window { percent: 90.0, resets_at: Some(t("2026-11-23T15:00:00Z")), severity: None };
        assert_eq!(reset_line(&soon, now, &utc, true), "Resets in 5h 0m");
    }

    #[test]
    fn tooltip_lists_both_windows() {
        let now = t("2026-11-23T10:00:00Z");
        let utc = FixedOffset::east_opt(0).unwrap();
        let usage = Usage {
            session: Some(Window { percent: 42.0, resets_at: Some(t("2026-11-23T12:14:00Z")), severity: None }),
            weekly: Some(Window { percent: 18.0, resets_at: Some(t("2026-11-30T09:00:00Z")), severity: None }),
            ..Usage::default()
        };
        assert_eq!(
            tooltip(&usage, now, &utc),
            "Session 42% · Resets in 2h 14m\nWeek 18% · Resets Mon 9:00 AM"
        );
        assert!(tooltip(&usage, now, &utc).len() < 127);
    }

    #[test]
    fn server_severity_can_only_raise_the_level() {
        let w = |percent, severity| Window { percent, resets_at: None, severity };
        assert_eq!(w(10.0, Some(Level::Critical)).level(), Level::Critical);
        assert_eq!(w(90.0, Some(Level::Ok)).level(), Level::Critical);
        assert_eq!(w(70.0, None).level(), Level::Warn);
        assert_eq!(parse_severity("normal"), Some(Level::Ok));
        assert_eq!(parse_severity("Warning"), Some(Level::Warn));
        assert_eq!(parse_severity("exceeded"), Some(Level::Critical));
        assert_eq!(parse_severity("brand_new"), None);
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

    #[test]
    fn limit_labels() {
        assert_eq!(limit_label("session"), "Session");
        assert_eq!(limit_label("weekly_all"), "Week");
        assert_eq!(limit_label("weekly_opus"), "Week · Opus");
        assert_eq!(limit_label("weekly_claude_code"), "Week · Claude Code");
        assert_eq!(limit_label("monthly_api"), "Monthly Api");
    }

    #[test]
    fn tooltip_adds_other_limits_that_fit() {
        let now = t("2026-11-23T10:00:00Z");
        let utc = FixedOffset::east_opt(0).unwrap();
        let mut usage = Usage { session: Some(Window { percent: 42.0, ..Window::default() }), ..Usage::default() };
        usage.others.push(Limit { kind: "weekly_opus".into(), window: Window { percent: 7.0, ..Window::default() } });
        assert_eq!(tooltip(&usage, now, &utc), "Session 42% · Not started\nWeek · Opus 7%");
        for _ in 0..20 {
            usage.others.push(usage.others[0].clone());
        }
        assert!(tooltip(&usage, now, &utc).chars().count() < 127);
    }

    #[test]
    fn breakdown_lines() {
        let shares = vec![
            Share { name: "Claude Code".into(), percent: 88 },
            Share { name: "Cowork".into(), percent: 12 },
        ];
        assert_eq!(breakdown_line(&shares).unwrap(), "This week: Claude Code 88% · Cowork 12%");
        assert_eq!(breakdown_line(&[]), None);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(30));
        assert_eq!(backoff(2), Duration::from_secs(60));
        assert_eq!(backoff(3), Duration::from_secs(120));
        assert_eq!(backoff(6), Duration::from_secs(600));
        assert_eq!(backoff(50), Duration::from_secs(600));
    }
}
