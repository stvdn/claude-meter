//! The app's state and the use cases that change it: sign in, sign out, apply a poll
//! result, change settings. The UI calls these and redraws; it holds no rules of its own.

use super::poller::{Command, Update};
use super::ports::{Autostart, FetchError, SessionStore, SettingsStore};
use super::settings::Settings;
use crate::domain::Usage;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Loading,
    Ok,
    /// The last update failed; numbers (if any) are from before.
    Failed(FetchError),
    SignedOut,
}

pub struct AppState {
    settings: Settings,
    usage: Option<Usage>,
    status: Status,
    /// The expired session, so the sign-in window doesn't hand it straight back.
    rejected_session: Option<String>,
    sessions: Box<dyn SessionStore>,
    settings_store: Box<dyn SettingsStore>,
    autostart: Box<dyn Autostart>,
}

impl AppState {
    pub fn new(
        sessions: Box<dyn SessionStore>,
        settings_store: Box<dyn SettingsStore>,
        autostart: Box<dyn Autostart>,
    ) -> Self {
        let settings = settings_store.load();
        let status = if sessions.load().is_some() { Status::Loading } else { Status::SignedOut };
        Self { settings, usage: None, status, rejected_session: None, sessions, settings_store, autostart }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }
    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn signed_in(&self) -> bool {
        self.status != Status::SignedOut
    }
    /// The numbers on screen are older than the last attempt to update them.
    pub fn stale(&self) -> bool {
        matches!(self.status, Status::Failed(_))
    }
    pub fn session(&self) -> Option<String> {
        self.sessions.load()
    }
    pub fn rejected_session(&self) -> Option<String> {
        self.rejected_session.clone()
    }

    /// Apply a poller update. Returns whether anything visible changed.
    pub fn apply(&mut self, update: Update) -> bool {
        match update {
            Update::Usage(u) => {
                self.usage = Some(u);
                self.status = Status::Ok;
            }
            Update::OrgId(o) => {
                self.settings.org_id = Some(o);
                self.save_settings();
                return false;
            }
            Update::Error(e) if e.needs_login() => {
                // The poller already stopped. Forget the session, but remember it so
                // the sign-in window ignores it if its browser profile still has it.
                self.rejected_session = self.sessions.load();
                self.sessions.clear();
                self.status = Status::SignedOut;
            }
            Update::Error(e) => self.status = Status::Failed(e),
        }
        true
    }

    /// The sign-in window produced a session. Tell the poller with the returned command.
    pub fn sign_in(&mut self, session: String) -> (Command, anyhow::Result<()>) {
        let saved = self.sessions.save(&session);
        self.rejected_session = None;
        self.status = Status::Loading;
        (Command::Session(Some(session)), saved)
    }

    pub fn sign_out(&mut self) -> Command {
        self.sessions.clear();
        self.settings.org_id = None;
        self.save_settings();
        self.usage = None;
        self.rejected_session = None;
        self.status = Status::SignedOut;
        Command::Session(None)
    }

    pub fn toggle_badge(&mut self) {
        self.settings.show_badge = !self.settings.show_badge;
        self.save_settings();
    }

    pub fn toggle_compact(&mut self) {
        self.settings.compact = !self.settings.compact;
        self.save_settings();
    }

    pub fn autostart_enabled(&self) -> bool {
        self.autostart.is_enabled()
    }

    pub fn toggle_autostart(&self) -> anyhow::Result<()> {
        self.autostart.set(!self.autostart.is_enabled())
    }

    /// Remember where the badge was dragged. Returns whether it moved; saving is left to
    /// the caller, so a drag doesn't write the file on every pixel.
    pub fn set_badge_position(&mut self, x: i32, y: i32) -> bool {
        let moved = (self.settings.badge_x, self.settings.badge_y) != (Some(x), Some(y));
        self.settings.badge_x = Some(x);
        self.settings.badge_y = Some(y);
        moved
    }

    pub fn save_settings(&self) {
        self.settings_store.save(&self.settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// In-memory stores; the `Rc` lets the test look inside after handing them over.
    #[derive(Clone, Default)]
    struct Memory {
        session: Rc<RefCell<Option<String>>>,
        settings: Rc<RefCell<Settings>>,
        autostart: Rc<RefCell<bool>>,
    }

    impl SessionStore for Memory {
        fn load(&self) -> Option<String> {
            self.session.borrow().clone()
        }
        fn save(&self, key: &str) -> anyhow::Result<()> {
            *self.session.borrow_mut() = Some(key.into());
            Ok(())
        }
        fn clear(&self) {
            *self.session.borrow_mut() = None;
        }
    }

    impl SettingsStore for Memory {
        fn load(&self) -> Settings {
            self.settings.borrow().clone()
        }
        fn save(&self, s: &Settings) {
            *self.settings.borrow_mut() = s.clone();
        }
    }

    impl Autostart for Memory {
        fn is_enabled(&self) -> bool {
            *self.autostart.borrow()
        }
        fn set(&self, enabled: bool) -> anyhow::Result<()> {
            *self.autostart.borrow_mut() = enabled;
            Ok(())
        }
    }

    fn app(session: Option<&str>) -> (AppState, Memory) {
        let mem = Memory::default();
        *mem.session.borrow_mut() = session.map(String::from);
        let state = AppState::new(Box::new(mem.clone()), Box::new(mem.clone()), Box::new(mem.clone()));
        (state, mem)
    }

    #[test]
    fn starts_loading_only_with_a_saved_session() {
        assert_eq!(app(Some("key")).0.status(), &Status::Loading);
        assert_eq!(app(None).0.status(), &Status::SignedOut);
    }

    #[test]
    fn usage_update_shows_numbers() {
        let (mut s, _) = app(Some("key"));
        assert!(s.apply(Update::Usage(Usage::default())));
        assert_eq!(s.status(), &Status::Ok);
        assert!(s.usage().is_some());
    }

    #[test]
    fn org_id_is_cached_without_a_redraw() {
        let (mut s, mem) = app(Some("key"));
        assert!(!s.apply(Update::OrgId("org-1".into())));
        assert_eq!(mem.settings.borrow().org_id.as_deref(), Some("org-1"));
    }

    #[test]
    fn expired_session_signs_out_and_is_remembered_as_rejected() {
        let (mut s, mem) = app(Some("old"));
        s.apply(Update::Error(FetchError::Unauthorized));
        assert_eq!(s.status(), &Status::SignedOut);
        assert_eq!(*mem.session.borrow(), None);
        assert_eq!(s.rejected_session().as_deref(), Some("old"));
    }

    #[test]
    fn other_errors_keep_the_old_numbers_as_stale() {
        let (mut s, _) = app(Some("key"));
        s.apply(Update::Usage(Usage::default()));
        s.apply(Update::Error(FetchError::Blocked));
        assert!(s.stale());
        assert!(s.usage().is_some());
    }

    #[test]
    fn sign_in_saves_the_session_and_tells_the_poller() {
        let (mut s, mem) = app(None);
        let (cmd, saved) = s.sign_in("new".into());
        assert!(saved.is_ok());
        assert_eq!(cmd, Command::Session(Some("new".into())));
        assert_eq!(mem.session.borrow().as_deref(), Some("new"));
        assert_eq!(s.status(), &Status::Loading);
    }

    #[test]
    fn sign_out_forgets_session_org_and_numbers() {
        let (mut s, mem) = app(Some("key"));
        s.apply(Update::OrgId("org-1".into()));
        s.apply(Update::Usage(Usage::default()));
        assert_eq!(s.sign_out(), Command::Session(None));
        assert_eq!(*mem.session.borrow(), None);
        assert_eq!(mem.settings.borrow().org_id, None);
        assert!(s.usage().is_none());
        assert!(!s.signed_in());
    }

    #[test]
    fn settings_toggles_are_saved() {
        let (mut s, mem) = app(None);
        s.toggle_compact();
        assert!(mem.settings.borrow().compact);
        s.toggle_autostart().unwrap();
        assert!(s.autostart_enabled());
        assert!(s.set_badge_position(10, 20));
        assert!(!s.set_badge_position(10, 20));
    }
}
