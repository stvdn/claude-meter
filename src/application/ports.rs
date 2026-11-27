//! Ports: what the application needs from the outside world, as traits.
//!
//! The application layer owns these; `infrastructure` implements them (claude.ai over HTTP,
//! Windows Credential Manager, a TOML file, the registry), and tests implement them with fakes.

use super::settings::Settings;
use crate::domain::Usage;

/// Why reading usage failed, in terms the application can act on.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FetchError {
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

impl FetchError {
    /// Errors that a new sign-in fixes, as opposed to ones that retrying fixes.
    pub fn needs_login(&self) -> bool {
        matches!(self, FetchError::Unauthorized)
    }
}

/// Where usage comes from.
pub trait UsageSource {
    /// The organization whose usage to read.
    fn org_id(&self, session_key: &str) -> Result<String, FetchError>;
    /// The unparsed response, for `--once` diagnostics.
    fn usage_raw(&self, session_key: &str, org_id: &str) -> Result<String, FetchError>;
    fn parse_usage(&self, raw: &str) -> Result<Usage, FetchError>;

    fn usage(&self, session_key: &str, org_id: &str) -> Result<Usage, FetchError> {
        self.parse_usage(&self.usage_raw(session_key, org_id)?)
    }
}

impl<T: UsageSource + ?Sized> UsageSource for Box<T> {
    fn org_id(&self, session_key: &str) -> Result<String, FetchError> {
        (**self).org_id(session_key)
    }
    fn usage_raw(&self, session_key: &str, org_id: &str) -> Result<String, FetchError> {
        (**self).usage_raw(session_key, org_id)
    }
    fn parse_usage(&self, raw: &str) -> Result<Usage, FetchError> {
        (**self).parse_usage(raw)
    }
}

/// Where the signed-in session is kept between runs.
pub trait SessionStore {
    fn load(&self) -> Option<String>;
    fn save(&self, session_key: &str) -> anyhow::Result<()>;
    fn clear(&self);
}

/// Where settings are kept between runs.
pub trait SettingsStore {
    fn load(&self) -> Settings;
    fn save(&self, settings: &Settings);
}

/// "Start with Windows" (or the equivalent on another OS).
pub trait Autostart {
    fn is_enabled(&self) -> bool;
    fn set(&self, enabled: bool) -> anyhow::Result<()>;
}
