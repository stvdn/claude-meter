//! The `Autostart` adapter: a value under HKCU\...\Run pointing at this exe. Per-user, no admin.

use crate::application::ports::Autostart;
use anyhow::{Context, Result};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "claude-meter";

pub struct RegistryAutostart;

impl Autostart for RegistryAutostart {
    fn is_enabled(&self) -> bool {
        windows_registry::CURRENT_USER.open(RUN_KEY).and_then(|k| k.get_string(VALUE)).is_ok()
    }

    fn set(&self, enabled: bool) -> Result<()> {
        let key = windows_registry::CURRENT_USER.create(RUN_KEY).context("opening the Run registry key")?;
        if enabled {
            let exe = std::env::current_exe().context("finding claude-meter.exe")?;
            key.set_string(VALUE, format!("\"{}\"", exe.display())).context("writing the Run registry value")
        } else {
            match key.remove_value(VALUE) {
                Ok(()) => Ok(()),
                Err(_) if !self.is_enabled() => Ok(()),
                Err(e) => Err(e).context("removing the Run registry value"),
            }
        }
    }
}
