//! The `SessionStore` adapter: the claude.ai session cookie in Windows Credential Manager
//! (Control Panel → Credential Manager → Windows Credentials → "claude-meter").
//! It is never written to a plain file or printed.

use crate::application::ports::SessionStore;
use anyhow::{Context, Result};

const SERVICE: &str = "claude-meter";
const ACCOUNT: &str = "claude.ai sessionKey";

pub struct CredentialStore;

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).context("opening Windows Credential Manager")
}

impl SessionStore for CredentialStore {
    fn load(&self) -> Option<String> {
        entry().ok()?.get_password().ok().filter(|k| !k.is_empty())
    }

    fn save(&self, key: &str) -> Result<()> {
        entry()?.set_password(key).context("saving the session to Credential Manager")
    }

    fn clear(&self) {
        if let Ok(e) = entry() {
            let _ = e.delete_credential();
        }
    }
}
