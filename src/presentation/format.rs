//! Text shown to the person: percentages, countdowns, tooltip and menu lines.
//! Pure functions, generic over the timezone so tests can pin one.

use crate::domain::{Share, Usage, Window};
use chrono::{DateTime, TimeZone, Utc};
use std::fmt::Display;

/// `42.4` -> `"42%"`. Clamped at 0 so a stray negative never shows.
pub fn percent_label(percent: f32) -> String {
    format!("{}%", percent.max(0.0).round() as i64)
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

/// Weekday and clock time in `tz`: `"Mon 9:00 AM"`.
pub fn weekday_time<Tz: TimeZone>(t: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    t.with_timezone(tz).format("%a %-I:%M %p").to_string()
}

/// Second line under a bar: "Resets in 2h 14m" for the session, "Resets Mon 9:00 AM" for the week.
pub fn reset_line<Tz: TimeZone>(window: &Window, now: DateTime<Utc>, tz: &Tz, weekly: bool) -> String
where
    Tz::Offset: Display,
{
    match window.resets_at {
        None => "Not started".to_string(),
        Some(t) if weekly && (t - now).num_hours() >= 24 => format!("Resets {}", weekday_time(t, tz)),
        Some(t) => format!("Resets in {}", countdown(now, t)),
    }
}

/// Tray tooltip text (Windows caps tooltips at 127 characters, this stays under).
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
        let line = format!("{} {}", limit_label(&l.kind), percent_label(l.window.percent));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Limit;
    use chrono::FixedOffset;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
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
        assert_eq!(reset_line(&session, now, &utc, false), "Resets in 2h 14m");
        assert_eq!(reset_line(&week, now, &utc, true), "Resets Mon 9:00 AM");
        assert_eq!(reset_line(&Window::default(), now, &utc, false), "Not started");
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
        assert_eq!(tooltip(&usage, now, &utc), "Session 42% · Resets in 2h 14m\nWeek 18% · Resets Mon 9:00 AM");
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
    fn limit_labels() {
        assert_eq!(limit_label("session"), "Session");
        assert_eq!(limit_label("weekly_all"), "Week");
        assert_eq!(limit_label("weekly_opus"), "Week · Opus");
        assert_eq!(limit_label("weekly_claude_code"), "Week · Claude Code");
        assert_eq!(limit_label("monthly_api"), "Monthly Api");
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
}
