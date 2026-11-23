//! The claude.ai session cookie, kept in Windows Credential Manager
//! (Control Panel → Credential Manager → Windows Credentials → "claude-meter").
//! It is never written to a plain file or printed.

use anyhow::{Context, Result};

const SERVICE: &str = "claude-meter";
const ACCOUNT: &str = "claude.ai sessionKey";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).context("opening Windows Credential Manager")
}

pub fn load() -> Option<String> {
    entry().ok()?.get_password().ok().filter(|k| !k.is_empty())
}

pub fn save(key: &str) -> Result<()> {
    entry()?.set_password(key).context("saving the session to Credential Manager")
}

pub fn clear() {
    if let Ok(e) = entry() {
        let _ = e.delete_credential();
    }
}
