//! Infrastructure layer: adapters that implement the application's ports with real
//! technology (HTTP, Windows Credential Manager, a TOML file, the registry).

pub mod autostart;
pub mod claude_api;
pub mod credential_store;
pub mod settings_file;
