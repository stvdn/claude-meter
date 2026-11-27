// Release builds are GUI apps (no console window behind the tray icon). The command-line
// modes attach to the terminal they were started from instead.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Composition root: the only place that knows every layer. It picks the real adapters
//! from `infrastructure` and hands them to `presentation`, which sees only the ports.
//!
//! ```text
//! presentation ──► application ──► domain
//!                       ▲
//! infrastructure ───────┘   (implements application::ports)
//! ```

mod application;
mod domain;
mod infrastructure;
mod presentation;

use infrastructure::autostart::RegistryAutostart;
use infrastructure::claude_api::ClaudeApi;
use infrastructure::credential_store::CredentialStore;
use infrastructure::settings_file::{self, SettingsFile};
use presentation::{cli, tray};
use std::path::PathBuf;

/// The sign-in window's own browser profile, kept apart from any real browser.
fn webview_profile() -> PathBuf {
    settings_file::data_dir().join("webview")
}

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None => {
            if !single_instance() {
                return; // Already running: its tray icon is the one to use.
            }
            tray::run(tray::Deps {
                source: Box::new(ClaudeApi::new()),
                sessions: Box::new(CredentialStore),
                settings: Box::new(SettingsFile::standard()),
                autostart: Box::new(RegistryAutostart),
                webview_profile: webview_profile(),
            });
        }
        Some("--once") => {
            attach_console();
            let code = cli::run_once(&ClaudeApi::new(), &CredentialStore, &SettingsFile::standard(), &webview_profile());
            std::process::exit(code)
        }
        Some("--logout") => {
            attach_console();
            cli::logout(&CredentialStore, &webview_profile());
        }
        Some(_) => {
            attach_console();
            println!("usage: claude-meter [--once | --logout]\n  (no arguments: run in the system tray)");
        }
    }
}

/// Hold a named mutex for the life of the process; `false` if another copy already has it.
fn single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\claude-meter-single-instance\0".encode_utf16().collect();
    // SAFETY: valid NUL-terminated name; the handle is deliberately never closed, so the
    // mutex lives exactly as long as this process.
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// In a release (GUI) build, print to the terminal that launched us, if any.
fn attach_console() {
    #[cfg(not(debug_assertions))]
    // SAFETY: plain Win32 call with a constant argument; failure just means no console.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}
