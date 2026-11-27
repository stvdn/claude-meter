//! Presentation layer: everything the person sees or types (tray icon, menu, badge,
//! sign-in window, command line). Depends on `application` and `domain`, never on
//! `infrastructure`: `main` hands it the adapters.

mod badge;
pub mod cli;
mod format;
mod login_window;
mod render;
pub mod tray;
