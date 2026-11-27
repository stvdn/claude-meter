//! The `UsageSource` adapter for claude.ai: HTTP client plus pure parsing functions.
//!
//! The endpoints are the ones claude.ai's own Settings → Usage page calls. They are
//! not a public, documented API, so parsing is deliberately tolerant: unknown fields
//! are ignored and a missing window becomes `None` instead of an error.

use crate::application::ports::{FetchError, UsageSource};
use crate::domain::{Level, Limit, Share, Usage, Window};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::time::Duration;

const BASE: &str = "https://claude.ai";
/// claude.ai sits behind bot protection that rejects unusual clients, so present as a browser.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

/// Map claude.ai's `severity` strings. Unknown words give `None`, so our thresholds decide.
fn parse_severity(s: &str) -> Option<Level> {
    match s.to_ascii_lowercase().as_str() {
        "normal" | "ok" | "low" => Some(Level::Ok),
        "warning" | "warn" | "elevated" | "approaching" | "medium" => Some(Level::Warn),
        "critical" | "high" | "exceeded" | "limited" | "blocked" | "error" => Some(Level::Critical),
        _ => None,
    }
}

/// Decide what an HTTP response means. Pure, so it's unit-tested.
pub fn classify(status: u16, cf_mitigated: bool, body: &str) -> Result<(), FetchError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let challenge = cf_mitigated || body.contains("challenge-platform") || body.contains("Just a moment");
    match status {
        _ if challenge => Err(FetchError::Blocked),
        401 | 403 => Err(FetchError::Unauthorized),
        s => Err(FetchError::Http(s)),
    }
}

/// Parse `/api/organizations/{id}/usage`. Expected shape (extra fields are ignored):
/// ```text
/// {"five_hour": {"utilization": 42.0, "resets_at": "2026-...Z"}, "seven_day": {...},
///  "limits": [{"kind": "session", "percent": 42, "severity": "normal", "resets_at": ...}, ...],
///  "seven_day_breakdown": {"rows": [{"display_name": "Claude Code", "percent": 88}, ...]}}
/// ```
/// `five_hour`/`seven_day` give the precise percentages; `limits` adds claude.ai's severity
/// and any extra limits (e.g. a weekly cap on one model). Either source alone is enough.
pub fn parse_usage(body: &str) -> Result<Usage, FetchError> {
    let v: Value = serde_json::from_str(body).map_err(|e| FetchError::Parse(e.to_string()))?;
    let mut limits = parse_limits(&v);
    let mut take = |kind: &str| limits.iter().position(|l| l.kind == kind).map(|i| limits.remove(i).window);
    let session = merge(parse_window(&v, "five_hour"), take("session"));
    let weekly = merge(parse_window(&v, "seven_day"), take("weekly_all"));
    if session.is_none() && weekly.is_none() {
        return Err(FetchError::Parse("no session or weekly limit in response".into()));
    }
    Ok(Usage { session, weekly, others: limits, breakdown: parse_breakdown(&v) })
}

/// Prefer the dedicated window's numbers, but keep the `limits` entry's severity.
fn merge(window: Option<Window>, limit: Option<Window>) -> Option<Window> {
    match (window, limit) {
        (Some(w), Some(l)) => Some(Window { severity: l.severity, resets_at: w.resets_at.or(l.resets_at), ..w }),
        (w, l) => w.or(l),
    }
}

fn parse_time(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// A key that's present but `null` means the window hasn't started yet, so it's 0% used.
fn parse_window(v: &Value, key: &str) -> Option<Window> {
    match v.get(key)? {
        Value::Null => Some(Window::default()),
        Value::Object(obj) => {
            let percent = obj.get("utilization").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            Some(Window { percent, resets_at: parse_time(obj.get("resets_at")), severity: None })
        }
        _ => None,
    }
}

/// The `limits` list. Entries without a `kind` are skipped; a missing list is empty.
fn parse_limits(v: &Value) -> Vec<Limit> {
    let Some(items) = v.get("limits").and_then(Value::as_array) else { return Vec::new() };
    items
        .iter()
        .filter_map(|l| {
            let kind = l.get("kind").and_then(Value::as_str).filter(|k| !k.is_empty())?;
            let percent = l
                .get("percent")
                .or_else(|| l.get("utilization"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32;
            let severity = l.get("severity").and_then(Value::as_str).and_then(parse_severity);
            let window = Window { percent, resets_at: parse_time(l.get("resets_at")), severity };
            Some(Limit { kind: kind.to_string(), window })
        })
        .collect()
}

/// `seven_day_breakdown.rows`: each product's share of this week's usage, largest first.
fn parse_breakdown(v: &Value) -> Vec<Share> {
    let Some(rows) = v.pointer("/seven_day_breakdown/rows").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut shares: Vec<Share> = rows
        .iter()
        .filter_map(|r| {
            let name = r.get("display_name").or_else(|| r.get("key")).and_then(Value::as_str)?;
            let percent = r.get("percent").and_then(Value::as_f64)?.clamp(0.0, 100.0).round() as u8;
            (percent > 0).then(|| Share { name: name.to_string(), percent })
        })
        .collect();
    shares.sort_by(|a, b| b.percent.cmp(&a.percent));
    shares
}

/// Parse `/api/organizations` and pick the org to read usage from.
/// Prefers an org that can chat (the personal Pro org); falls back to the first one.
pub fn parse_org_id(body: &str) -> Result<String, FetchError> {
    let v: Value = serde_json::from_str(body).map_err(|e| FetchError::Parse(e.to_string()))?;
    let orgs = v.as_array().ok_or_else(|| FetchError::Parse("organizations is not a list".into()))?;
    let can_chat = |o: &&Value| {
        o.get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|caps| caps.iter().any(|c| c.as_str() == Some("chat")))
    };
    let org = orgs.iter().find(can_chat).or_else(|| orgs.first());
    let id = org
        .and_then(|o| o.get("uuid"))
        .and_then(Value::as_str)
        .ok_or_else(|| FetchError::Parse("no organization uuid".into()))?;
    // The id goes into a URL path, so only accept UUID characters.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(FetchError::Parse("organization uuid has unexpected characters".into()));
    }
    Ok(id.to_string())
}

/// Blocking HTTP client. One request a minute doesn't need an async runtime.
pub struct ClaudeApi {
    agent: ureq::Agent,
}

impl Default for ClaudeApi {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeApi {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build();
        Self { agent: config.into() }
    }

    fn get(&self, path: &str, session_key: &str) -> Result<String, FetchError> {
        let mut resp = self
            .agent
            .get(format!("{BASE}{path}"))
            .header("Cookie", format!("sessionKey={session_key}"))
            .header("Accept", "application/json")
            .call()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let cf_mitigated = resp.headers().contains_key("cf-mitigated");
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        classify(status, cf_mitigated, &body)?;
        Ok(body)
    }

}

impl UsageSource for ClaudeApi {
    fn org_id(&self, session_key: &str) -> Result<String, FetchError> {
        parse_org_id(&self.get("/api/organizations", session_key)?)
    }

    fn usage_raw(&self, session_key: &str, org_id: &str) -> Result<String, FetchError> {
        self.get(&format!("/api/organizations/{org_id}/usage?via=plan"), session_key)
    }

    fn parse_usage(&self, raw: &str) -> Result<Usage, FetchError> {
        parse_usage(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Level;

    #[test]
    fn parses_both_windows() {
        let body = r#"{
            "five_hour": {"utilization": 42.0, "resets_at": "2026-11-23T12:14:00.123456+00:00"},
            "seven_day": {"utilization": 18, "resets_at": "2026-11-30T09:00:00Z"},
            "seven_day_opus": null,
            "something_new": {"x": 1}
        }"#;
        let u = parse_usage(body).unwrap();
        let s = u.session.unwrap();
        assert_eq!(s.percent, 42.0);
        assert_eq!(s.resets_at.unwrap().to_rfc3339(), "2026-11-23T12:14:00.123456+00:00");
        assert_eq!(u.weekly.unwrap().percent, 18.0);
    }

    #[test]
    fn null_window_means_not_started() {
        let u = parse_usage(r#"{"five_hour": null, "seven_day": {"utilization": 5}}"#).unwrap();
        assert_eq!(u.session, Some(Window::default()));
        assert_eq!(u.weekly.unwrap().resets_at, None);
    }

    /// Trimmed from a real response (2026-11-23).
    const REAL: &str = r#"{
        "five_hour": {"utilization": 19.0, "resets_at": "2026-11-23T23:40:00.999428+00:00", "limit_dollars": null},
        "seven_day": {"utilization": 4.0, "resets_at": "2026-11-29T19:00:00.999453+00:00"},
        "seven_day_opus": null,
        "limits": [
            {"kind": "session", "group": "session", "percent": 19, "severity": "normal",
             "resets_at": "2026-11-23T23:40:00.999428+00:00", "scope": null, "is_active": true},
            {"kind": "weekly_all", "group": "weekly", "percent": 4, "severity": "normal",
             "resets_at": "2026-11-29T19:00:00.999453+00:00", "scope": null, "is_active": false}
        ],
        "seven_day_breakdown": {"rows": [
            {"key": "claude_code", "display_name": "Claude Code", "percent": 88, "percent_of_limit": 4},
            {"key": "chat", "display_name": "Chats", "percent": 0, "percent_of_limit": 0},
            {"key": "cowork", "display_name": "Cowork", "percent": 12, "percent_of_limit": 0}
        ]}
    }"#;

    #[test]
    fn parses_real_response_with_limits() {
        let u = parse_usage(REAL).unwrap();
        let s = u.session.unwrap();
        assert_eq!((s.percent, s.severity), (19.0, Some(Level::Ok)));
        assert_eq!(u.weekly.unwrap().percent, 4.0);
        assert!(u.others.is_empty(), "session and weekly_all are not repeated as extras");
        assert_eq!(
            u.breakdown,
            vec![Share { name: "Claude Code".into(), percent: 88 }, Share { name: "Cowork".into(), percent: 12 }]
        );
    }

    #[test]
    fn extra_limits_and_severity() {
        let body = r#"{
            "five_hour": {"utilization": 30.0},
            "limits": [
                {"kind": "session", "percent": 30, "severity": "critical"},
                {"kind": "weekly_opus", "percent": 71.5, "severity": "warning", "resets_at": "2026-11-30T09:00:00Z"},
                {"percent": 5},
                {"kind": "weekly_all", "percent": 40, "severity": "something_new"}
            ]
        }"#;
        let u = parse_usage(body).unwrap();
        assert_eq!(u.session.as_ref().unwrap().level(), Level::Critical);
        // No seven_day object: the weekly limit comes from the list alone.
        let w = u.weekly.unwrap();
        assert_eq!((w.percent, w.severity), (40.0, None));
        assert_eq!(u.others.len(), 1);
        assert_eq!(u.others[0].kind, "weekly_opus");
        assert_eq!(u.others[0].window.level(), Level::Warn);
        assert!(u.others[0].window.resets_at.is_some());
    }

    #[test]
    fn limits_alone_are_enough() {
        let u = parse_usage(r#"{"limits": [{"kind": "session", "percent": 12}]}"#).unwrap();
        assert_eq!(u.session.unwrap().percent, 12.0);
        assert!(u.weekly.is_none());
    }

    #[test]
    fn missing_windows_is_an_error() {
        assert!(matches!(parse_usage(r#"{"foo": 1}"#), Err(FetchError::Parse(_))));
        assert!(matches!(parse_usage("<html>"), Err(FetchError::Parse(_))));
    }

    #[test]
    fn picks_chat_org() {
        let body = r#"[
            {"uuid": "11111111-aaaa", "capabilities": ["api"]},
            {"uuid": "22222222-bbbb", "capabilities": ["chat", "claude_pro"]}
        ]"#;
        assert_eq!(parse_org_id(body).unwrap(), "22222222-bbbb");
        assert_eq!(parse_org_id(r#"[{"uuid": "abc-123"}]"#).unwrap(), "abc-123");
    }

    #[test]
    fn rejects_bad_org_ids() {
        assert!(parse_org_id("[]").is_err());
        assert!(parse_org_id(r#"[{"uuid": "../../etc"}]"#).is_err());
        assert!(parse_org_id(r#"{"uuid": "abc"}"#).is_err());
    }

    #[test]
    fn classifies_responses() {
        assert_eq!(classify(200, false, "{}"), Ok(()));
        assert_eq!(classify(401, false, "{}"), Err(FetchError::Unauthorized));
        assert_eq!(classify(403, false, r#"{"error":"x"}"#), Err(FetchError::Unauthorized));
        assert_eq!(classify(403, true, ""), Err(FetchError::Blocked));
        assert_eq!(classify(403, false, "<title>Just a moment...</title>"), Err(FetchError::Blocked));
        assert_eq!(classify(500, false, ""), Err(FetchError::Http(500)));
        assert!(FetchError::Unauthorized.needs_login());
        assert!(!FetchError::Http(500).needs_login());
    }

    #[test]
    fn maps_severity_words() {
        assert_eq!(parse_severity("normal"), Some(Level::Ok));
        assert_eq!(parse_severity("Warning"), Some(Level::Warn));
        assert_eq!(parse_severity("exceeded"), Some(Level::Critical));
        assert_eq!(parse_severity("brand_new"), None);
    }
}
