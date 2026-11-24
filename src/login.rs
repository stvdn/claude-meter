//! One-time sign-in window.
//!
//! Opens claude.ai's normal login page in a WebView2 window. The person signs in the way
//! they always do; the app never sees the password. Once claude.ai sets its `sessionKey`
//! cookie, the app reads it from the webview's cookie store and the window is closed.
//! The webview (and its ~50 MB of browser processes) only exists while this window is open.

use crate::config;
use anyhow::{Context, Result};
use tao::dpi::LogicalSize;
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

const LOGIN_URL: &str = "https://claude.ai/login";
const COOKIE_URL: &str = "https://claude.ai";

/// wry's default WebView2 flags, plus `UserAgentClientHint` so the webview stops sending
/// `Sec-CH-UA: "Microsoft Edge WebView2"`. Google's sign-in refuses embedded browsers, and
/// that header (or a user agent that disagrees with it) is how it spots them.
const BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,UserAgentClientHint";

/// A desktop Edge user agent for the installed WebView2 version, e.g. `Edg/157.0.0.0`.
/// It has to match the real engine: a stale hard-coded version is itself a red flag.
fn edge_user_agent() -> Option<String> {
    let version = wry::webview_version().ok()?;
    let major = version.split('.').next().filter(|m| m.parse::<u32>().is_ok())?;
    Some(edge_user_agent_for(major))
}

fn edge_user_agent_for(major: &str) -> String {
    format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0"
    )
}

pub struct LoginWindow {
    // Field order is drop order: the webview must go before its window.
    webview: WebView,
    _context: WebContext,
    window: Window,
    /// A session already known to be expired; ignore it if the profile still holds it.
    reject: Option<String>,
}

impl LoginWindow {
    pub fn open<T: 'static>(target: &EventLoopWindowTarget<T>, reject: Option<String>) -> Result<Self> {
        let window = WindowBuilder::new()
            .with_title("Sign in to Claude · claude-meter (if Google refuses, use Continue with email)")
            .with_inner_size(LogicalSize::new(480.0, 720.0))
            .build(target)
            .context("creating the sign-in window")?;
        // Own browser profile, so the person's other browsers are untouched and the
        // sign-in survives restarts (re-login is usually one click).
        let mut context = WebContext::new(Some(config::data_dir().join("webview")));
        let mut builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(LOGIN_URL)
            .with_additional_browser_args(BROWSER_ARGS);
        // Present as regular desktop Edge (WebView2 is the same engine). If the version
        // can't be read, keep WebView2's own user agent rather than guess one.
        if let Some(ua) = edge_user_agent() {
            builder = builder.with_user_agent(&ua);
        }
        let webview = builder
            .build(&window)
            .context("starting WebView2 (is the WebView2 runtime installed?)")?;
        Ok(Self { webview, _context: context, window, reject })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// The session cookie once claude.ai has set it, else `None`.
    pub fn session_key(&self) -> Option<String> {
        let cookies = self.webview.cookies_for_url(COOKIE_URL).ok()?;
        cookies
            .iter()
            .find(|c| c.name() == "sessionKey")
            .map(|c| c.value().to_string())
            .filter(|v| !v.is_empty() && self.reject.as_deref() != Some(v.as_str()))
    }
}

/// Sign-out: forget the login window's browser profile too. Best effort, since
/// WebView2 can hold files open for a moment after its window closes.
pub fn clear_profile() {
    let _ = std::fs::remove_dir_all(config::data_dir().join("webview"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_uses_the_engine_version() {
        let ua = edge_user_agent_for("157");
        assert!(ua.contains("Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0"));
        assert!(!ua.contains("  "));
    }
}
