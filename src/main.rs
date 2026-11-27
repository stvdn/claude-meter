// Release builds are GUI apps (no console window behind the tray icon). The command-line
// modes attach to the terminal they were started from instead.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod autostart;
mod badge;
mod config;
mod login;
mod model;
mod render;
mod secrets;

use api::{ApiError, Client};
use config::Config;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        None => {
            if !single_instance() {
                return; // Already running: its tray icon is the one to use.
            }
            app::run();
        }
        Some("--once") => {
            attach_console();
            std::process::exit(run_once())
        }
        Some("--logout") => {
            attach_console();
            secrets::clear();
            login::clear_profile();
            println!("Signed out.");
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

/// `claude-meter --once`: sign in if needed, fetch usage one time, print it, exit.
/// Prints the raw JSON too, which is how the response shape was confirmed.
fn run_once() -> i32 {
    let mut cfg = Config::load();
    let client = Client::new();

    let key = match secrets::load() {
        Some(k) => k,
        None => {
            println!("Not signed in yet: opening the claude.ai sign-in window...");
            match login_blocking(None) {
                Some(k) => {
                    if let Err(e) = secrets::save(&k) {
                        eprintln!("warning: {e:#}");
                    }
                    k
                }
                None => {
                    eprintln!("Sign-in window closed before signing in.");
                    return 1;
                }
            }
        }
    };

    let result = (|| {
        let org = match cfg.org_id.clone() {
            Some(o) => o,
            None => {
                let o = client.org_id(&key)?;
                cfg.org_id = Some(o.clone());
                cfg.save();
                o
            }
        };
        let raw = client.usage_raw(&key, &org)?;
        Ok::<_, ApiError>(raw)
    })();

    match result {
        Ok(raw) => {
            println!("--- raw response ---\n{raw}\n--- parsed ---");
            match api::parse_usage(&raw) {
                Ok(u) => {
                    println!("{}", model::tooltip(&u, chrono::Utc::now(), &chrono::Local));
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    2
                }
            }
        }
        Err(e) if e.needs_login() => {
            secrets::clear();
            eprintln!("{e}. Run `claude-meter --once` again to sign in.");
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Show the sign-in window until claude.ai sets its session cookie (or the window is closed).
fn login_blocking(reject: Option<String>) -> Option<String> {
    let mut event_loop = EventLoop::new();
    let mut window = match login::LoginWindow::open(&event_loop, reject) {
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
                if let Some(k) = window.as_ref().and_then(|w| w.session_key()) {
                    found = Some(k);
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
