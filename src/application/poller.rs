//! The polling use case: read usage on a timer, back off on errors, stop when signed out.
//!
//! `Poller::step` is one decision and is unit-tested with a fake `UsageSource`;
//! `Poller::run` is the thin blocking loop around it that runs on its own thread.

use super::ports::{FetchError, UsageSource};
use crate::domain::Usage;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// What the poller reports back to the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Usage(Usage),
    /// A newly looked-up organization id, worth caching in settings.
    OrgId(String),
    Error(FetchError),
}

/// What the UI can tell the poller.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Refresh,
    /// A new session (after sign-in) or `None` (signed out).
    Session(Option<String>),
}

/// Wait before retrying after `failures` consecutive errors: 30 s, 60 s, 120 s ... capped at 10 min.
pub fn backoff(failures: u32) -> Duration {
    let secs = 30u64.saturating_mul(1u64 << failures.saturating_sub(1).min(10));
    Duration::from_secs(secs.min(600))
}

pub struct Poller<S: UsageSource> {
    source: S,
    session: Option<String>,
    org: Option<String>,
    interval: Duration,
    failures: u32,
}

impl<S: UsageSource> Poller<S> {
    pub fn new(source: S, session: Option<String>, org: Option<String>, interval: Duration) -> Self {
        Self { source, session, org, interval, failures: 0 }
    }

    /// Poll once. Returns what to report and how long to wait before the next poll
    /// (`None`: nothing to do until a command arrives, e.g. while signed out).
    pub fn step(&mut self) -> (Vec<Update>, Option<Duration>) {
        let Some(session) = self.session.clone() else { return (Vec::new(), None) };
        let mut updates = Vec::new();
        let org = match self.org.clone() {
            Some(o) => Ok(o),
            None => self.source.org_id(&session).inspect(|o| {
                self.org = Some(o.clone());
                updates.push(Update::OrgId(o.clone()));
            }),
        };
        let wait = match org.and_then(|o| self.source.usage(&session, &o)) {
            Ok(usage) => {
                self.failures = 0;
                updates.push(Update::Usage(usage));
                Some(self.interval)
            }
            Err(e) if e.needs_login() => {
                // Retrying can't help; wait for a new session.
                self.session = None;
                updates.push(Update::Error(e));
                None
            }
            Err(e) => {
                // A cached org that no longer exists: look it up again next time.
                if e == FetchError::Http(404) {
                    self.org = None;
                }
                self.failures += 1;
                updates.push(Update::Error(e));
                Some(backoff(self.failures))
            }
        };
        (updates, wait)
    }

    pub fn handle(&mut self, command: Command) {
        match command {
            Command::Refresh => {}
            Command::Session(s) => {
                self.session = s;
                self.failures = 0;
            }
        }
    }

    /// Poll until the command channel closes or `report` returns `false`.
    /// Blocking on purpose: one request a minute doesn't need an async runtime.
    pub fn run(mut self, commands: Receiver<Command>, mut report: impl FnMut(Update) -> bool) {
        loop {
            let (updates, wait) = self.step();
            for u in updates {
                if !report(u) {
                    return;
                }
            }
            let command = match wait {
                Some(d) => commands.recv_timeout(d),
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match command {
                Ok(c) => self.handle(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    /// Answers `usage` calls from a script and counts org lookups.
    #[derive(Default)]
    struct FakeSource {
        org_lookups: Cell<u32>,
        results: RefCell<VecDeque<Result<Usage, FetchError>>>,
    }

    impl FakeSource {
        fn scripted(results: Vec<Result<Usage, FetchError>>) -> Self {
            Self { results: RefCell::new(results.into()), ..Self::default() }
        }
    }

    impl UsageSource for FakeSource {
        fn org_id(&self, _: &str) -> Result<String, FetchError> {
            self.org_lookups.set(self.org_lookups.get() + 1);
            Ok("org-1".into())
        }
        fn usage_raw(&self, _: &str, _: &str) -> Result<String, FetchError> {
            unreachable!("the poller only uses `usage`")
        }
        fn parse_usage(&self, _: &str) -> Result<Usage, FetchError> {
            unreachable!("the poller only uses `usage`")
        }
        fn usage(&self, _: &str, org: &str) -> Result<Usage, FetchError> {
            assert_eq!(org, "org-1");
            self.results.borrow_mut().pop_front().expect("no scripted result left")
        }
    }

    const MINUTE: Duration = Duration::from_secs(60);

    fn poller(results: Vec<Result<Usage, FetchError>>) -> Poller<FakeSource> {
        Poller::new(FakeSource::scripted(results), Some("key".into()), None, MINUTE)
    }

    #[test]
    fn first_poll_looks_up_the_org_once() {
        let mut p = poller(vec![Ok(Usage::default()), Ok(Usage::default())]);
        let (updates, wait) = p.step();
        assert_eq!(updates, vec![Update::OrgId("org-1".into()), Update::Usage(Usage::default())]);
        assert_eq!(wait, Some(MINUTE));
        assert_eq!(p.step().0, vec![Update::Usage(Usage::default())]);
        assert_eq!(p.source.org_lookups.get(), 1);
    }

    #[test]
    fn expired_session_stops_polling_until_a_new_one() {
        let mut p = poller(vec![Err(FetchError::Unauthorized), Ok(Usage::default())]);
        let (updates, wait) = p.step();
        assert_eq!(updates.last(), Some(&Update::Error(FetchError::Unauthorized)));
        assert_eq!(wait, None);
        assert_eq!(p.step(), (vec![], None), "no requests while signed out");
        p.handle(Command::Session(Some("new-key".into())));
        assert_eq!(p.step().0, vec![Update::Usage(Usage::default())]);
    }

    #[test]
    fn network_errors_back_off_and_success_resets() {
        let net = || Err(FetchError::Network("offline".into()));
        let mut p = poller(vec![net(), net(), net(), Ok(Usage::default())]);
        assert_eq!(p.step().1, Some(Duration::from_secs(30)));
        assert_eq!(p.step().1, Some(Duration::from_secs(60)));
        assert_eq!(p.step().1, Some(Duration::from_secs(120)));
        assert_eq!(p.step().1, Some(MINUTE));
    }

    #[test]
    fn a_missing_org_is_looked_up_again() {
        let mut p = poller(vec![Err(FetchError::Http(404)), Ok(Usage::default())]);
        p.step();
        p.step();
        assert_eq!(p.source.org_lookups.get(), 2);
    }

    #[test]
    fn a_new_session_resets_the_backoff() {
        let net = || Err(FetchError::Network("offline".into()));
        let mut p = poller(vec![net(), net(), net()]);
        p.step();
        p.step();
        p.handle(Command::Session(Some("key".into())));
        assert_eq!(p.step().1, Some(Duration::from_secs(30)));
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(30));
        assert_eq!(backoff(2), Duration::from_secs(60));
        assert_eq!(backoff(3), Duration::from_secs(120));
        assert_eq!(backoff(6), Duration::from_secs(600));
        assert_eq!(backoff(50), Duration::from_secs(600));
    }
}
