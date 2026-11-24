//! claude.ai usage API: HTTP client plus pure parsing functions.
//!
//! The endpoints are the ones claude.ai's own Settings → Usage page calls. They are
//! not a public, documented API, so parsing is deliberately tolerant: unknown fields
//! are ignored and a missing window becomes `None` instead of an error.

use crate::model::{Usage, Window};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::time::Duration;

const BASE: &str = "https://claude.ai";
/// claude.ai sits behind bot protection that rejects unusual clients, so present as a browser.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ApiError {
    #[error("not signed in")]
    NoKey,
    #[error("sign-in expired")]
    Unauthorized,
    #[error("blocked by claude.ai bot protection")]
    Blocked,
    #[error("claude.ai returned HTTP {0}")]
    Http(u16),
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Parse(String),
}

impl ApiError {
    /// Errors that a new sign-in fixes, as opposed to ones that retrying fixes.
    pub fn needs_login(&self) -> bool {
        matches!(self, ApiError::NoKey | ApiError::Unauthorized)
    }
}

/// Decide what an HTTP response means. Pure, so it's unit-tested.
pub fn classify(status: u16, cf_mitigated: bool, body: &str) -> Result<(), ApiError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let challenge = cf_mitigated || body.contains("challenge-platform") || body.contains("Just a moment");
    match status {
        _ if challenge => Err(ApiError::Blocked),
        401 | 403 => Err(ApiError::Unauthorized),
        s => Err(ApiError::Http(s)),
    }
}

/// Parse `/api/organizations/{id}/usage`. Expected shape (extra fields are ignored):
/// `{"five_hour": {"utilization": 42.0, "resets_at": "2026-...Z"}, "seven_day": {...}}`
pub fn parse_usage(body: &str) -> Result<Usage, ApiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| ApiError::Parse(e.to_string()))?;
    let session = parse_window(&v, "five_hour");
    let weekly = parse_window(&v, "seven_day");
    if session.is_none() && weekly.is_none() {
        return Err(ApiError::Parse("no five_hour or seven_day window in response".into()));
    }
    Ok(Usage { session, weekly, ..Usage::default() })
}

/// A key that's present but `null` means the window hasn't started yet, so it's 0% used.
fn parse_window(v: &Value, key: &str) -> Option<Window> {
    match v.get(key)? {
        Value::Null => Some(Window::default()),
        Value::Object(obj) => {
            let percent = obj.get("utilization").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let resets_at = obj
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            Some(Window { percent, resets_at, severity: None })
        }
        _ => None,
    }
}

/// Parse `/api/organizations` and pick the org to read usage from.
/// Prefers an org that can chat (the personal Pro org); falls back to the first one.
pub fn parse_org_id(body: &str) -> Result<String, ApiError> {
    let v: Value = serde_json::from_str(body).map_err(|e| ApiError::Parse(e.to_string()))?;
    let orgs = v.as_array().ok_or_else(|| ApiError::Parse("organizations is not a list".into()))?;
    let can_chat = |o: &&Value| {
        o.get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|caps| caps.iter().any(|c| c.as_str() == Some("chat")))
    };
    let org = orgs.iter().find(can_chat).or_else(|| orgs.first());
    let id = org
        .and_then(|o| o.get("uuid"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::Parse("no organization uuid".into()))?;
    // The id goes into a URL path, so only accept UUID characters.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(ApiError::Parse("organization uuid has unexpected characters".into()));
    }
    Ok(id.to_string())
}

/// Blocking HTTP client. One request a minute doesn't need an async runtime.
pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build();
        Self { agent: config.into() }
    }

    fn get(&self, path: &str, session_key: &str) -> Result<String, ApiError> {
        let mut resp = self
            .agent
            .get(format!("{BASE}{path}"))
            .header("Cookie", format!("sessionKey={session_key}"))
            .header("Accept", "application/json")
            .call()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let cf_mitigated = resp.headers().contains_key("cf-mitigated");
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        classify(status, cf_mitigated, &body)?;
        Ok(body)
    }

    pub fn org_id(&self, session_key: &str) -> Result<String, ApiError> {
        parse_org_id(&self.get("/api/organizations", session_key)?)
    }

    /// Raw JSON of the usage endpoint (what `--once` prints).
    pub fn usage_raw(&self, session_key: &str, org_id: &str) -> Result<String, ApiError> {
        self.get(&format!("/api/organizations/{org_id}/usage?via=plan"), session_key)
    }

    pub fn usage(&self, session_key: &str, org_id: &str) -> Result<Usage, ApiError> {
        parse_usage(&self.usage_raw(session_key, org_id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn missing_windows_is_an_error() {
        assert!(matches!(parse_usage(r#"{"foo": 1}"#), Err(ApiError::Parse(_))));
        assert!(matches!(parse_usage("<html>"), Err(ApiError::Parse(_))));
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
        assert_eq!(classify(401, false, "{}"), Err(ApiError::Unauthorized));
        assert_eq!(classify(403, false, r#"{"error":"x"}"#), Err(ApiError::Unauthorized));
        assert_eq!(classify(403, true, ""), Err(ApiError::Blocked));
        assert_eq!(classify(403, false, "<title>Just a moment...</title>"), Err(ApiError::Blocked));
        assert_eq!(classify(500, false, ""), Err(ApiError::Http(500)));
        assert!(ApiError::Unauthorized.needs_login());
        assert!(!ApiError::Http(500).needs_login());
    }
}
