//! Command-line modes: `--once` (check usage one time and print it) and `--logout`.

use super::format;
use super::login_window::{self, LoginWindow};
use crate::application::ports::{FetchError, SessionStore, SettingsStore, UsageSource};
use std::path::Path;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;

/// `claude-meter --once`: sign in if needed, fetch usage one time, print it, exit.
/// Prints the raw JSON too, which is how the response shape was confirmed.
pub fn run_once(
    source: &dyn UsageSource,
    sessions: &dyn SessionStore,
    settings_store: &dyn SettingsStore,
    webview_profile: &Path,
) -> i32 {
    let mut settings = settings_store.load();

    let session = match sessions.load() {
        Some(s) => s,
        None => {
            println!("Not signed in yet: opening the claude.ai sign-in window...");
            match login_blocking(webview_profile, None) {
                Some(s) => {
                    if let Err(e) = sessions.save(&s) {
                        eprintln!("warning: {e:#}");
                    }
                    s
                }
                None => {
                    eprintln!("Sign-in window closed before signing in.");
                    return 1;
                }
            }
        }
    };

    let result = (|| {
        let org = match settings.org_id.clone() {
            Some(o) => o,
            None => {
                let o = source.org_id(&session)?;
                settings.org_id = Some(o.clone());
                settings_store.save(&settings);
                o
            }
        };
        source.usage_raw(&session, &org)
    })();

    match result {
        Ok(raw) => {
            println!("--- raw response ---\n{raw}\n--- parsed ---");
            match source.parse_usage(&raw) {
                Ok(u) => {
                    println!("{}", format::tooltip(&u, chrono::Utc::now(), &chrono::Local));
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        Err(e @ FetchError::Unauthorized) => {
            sessions.clear();
            eprintln!("{e}. Run `claude-meter --once` again to sign in.");
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// `claude-meter --logout`: forget the session and the sign-in window's browser profile.
pub fn logout(sessions: &dyn SessionStore, webview_profile: &Path) {
    sessions.clear();
    login_window::clear_profile(webview_profile);
    println!("Signed out.");
}

/// Show the sign-in window until claude.ai sets its session cookie (or the window is closed).
fn login_blocking(webview_profile: &Path, reject: Option<String>) -> Option<String> {
    let mut event_loop = EventLoop::new();
    let mut window = match LoginWindow::open(&event_loop, webview_profile, reject) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("{e:#}");
            return None;
        }
    };
    let mut found = None;
    event_loop.run_return(|event, _, control_flow| {
        // Wake once a second to check the cookie; otherwise sleep.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                if let Some(s) = window.as_ref().and_then(|w| w.session_key()) {
                    found = Some(s);
                    window = None;
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
    drop(window);
    found
}
