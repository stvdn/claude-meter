//! The tray app: one event loop on the main thread, one worker thread that polls claude.ai.
//!
//! The worker blocks on HTTP and sleeps between polls, so the UI thread never waits on the
//! network and idles at ~0% CPU. The sign-in webview exists only while its window is open.

use crate::api::{ApiError, Client};
use crate::badge::{self, Badge};
use crate::config::Config;
use crate::login::LoginWindow;
use crate::model::{self, Usage};
use crate::render::{self, IconState};
use crate::{autostart, login, secrets};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use tao::dpi::PhysicalPosition;
use tao::event::{ElementState, Event, MouseButton, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tray_icon::menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton as TrayButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

const USAGE_URL: &str = "https://claude.ai/settings/usage";
const ICON_SIZE: u32 = 32;

#[derive(Debug)]
pub enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Fetched(Fetched),
}

#[derive(Debug)]
pub enum Fetched {
    Usage(Usage),
    OrgId(String),
    Error(ApiError),
}

enum Cmd {
    Refresh,
    /// A new session key (after sign-in) or `None` (signed out).
    Key(Option<String>),
}

/// Poll claude.ai until the main thread hangs up. Blocking on purpose: no async runtime.
fn worker(proxy: EventLoopProxy<UserEvent>, rx: Receiver<Cmd>, mut key: Option<String>, mut org: Option<String>, interval: Duration) {
    let client = Client::new();
    let send = |f: Fetched| proxy.send_event(UserEvent::Fetched(f)).is_ok();
    let mut failures = 0u32;
    loop {
        let wait = match key.as_deref() {
            None => None,
            Some(k) => {
                let result = (|| {
                    let o = match &org {
                        Some(o) => o.clone(),
                        None => {
                            let o = client.org_id(k)?;
                            send(Fetched::OrgId(o.clone()));
                            org = Some(o.clone());
                            o
                        }
                    };
                    client.usage(k, &o)
                })();
                match result {
                    Ok(u) => {
                        failures = 0;
                        if !send(Fetched::Usage(u)) {
                            return;
                        }
                        Some(interval)
                    }
                    Err(e) if e.needs_login() => {
                        key = None;
                        if !send(Fetched::Error(e)) {
                            return;
                        }
                        None
                    }
                    Err(e) => {
                        // A cached org that no longer exists: look it up again next time.
                        if matches!(e, ApiError::Http(404)) {
                            org = None;
                        }
                        failures += 1;
                        if !send(Fetched::Error(e)) {
                            return;
                        }
                        Some(model::backoff(failures))
                    }
                }
            }
        };
        let cmd = match wait {
            Some(d) => rx.recv_timeout(d),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match cmd {
            Ok(Cmd::Refresh) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Cmd::Key(k)) => {
                key = k;
                failures = 0;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Menu item ids. Items are rebuilt on every update, so ids are fixed strings.
mod id {
    pub const REFRESH: &str = "refresh";
    pub const SIGN_IN: &str = "sign-in";
    pub const SIGN_OUT: &str = "sign-out";
    pub const OPEN: &str = "open";
    pub const BADGE: &str = "badge";
    pub const COMPACT: &str = "compact";
    pub const AUTOSTART: &str = "autostart";
    pub const QUIT: &str = "quit";
}

enum Status {
    Loading,
    Ok,
    /// The last update failed; numbers (if any) are from before.
    Failed(ApiError),
    SignedOut,
}

struct App {
    cfg: Config,
    cfg_dirty: Option<Instant>,
    tx: Sender<Cmd>,
    tray: TrayIcon,
    menu: Menu,
    usage: Option<Usage>,
    status: Status,
    login: Option<LoginWindow>,
    /// The expired key, so the sign-in window doesn't hand it straight back.
    rejected_key: Option<String>,
    badge: Option<Badge>,
}

impl App {
    fn signed_in(&self) -> bool {
        !matches!(self.status, Status::SignedOut)
    }

    fn icon_state(&self) -> IconState {
        match (&self.status, &self.usage) {
            (Status::SignedOut, _) => IconState::SignedOut,
            (Status::Failed(_), Some(u)) => IconState::from_usage(u, true),
            (_, Some(u)) => IconState::from_usage(u, false),
            (_, None) => IconState::Loading,
        }
    }

    fn status_line(&self) -> Option<String> {
        match &self.status {
            Status::Loading => Some("Loading…".into()),
            Status::Ok => None,
            Status::Failed(ApiError::Blocked) => Some("⚠ claude.ai blocked the request · retrying".into()),
            Status::Failed(e) => Some(format!("⚠ Couldn't update ({e}) · retrying")),
            Status::SignedOut => Some("Signed out · choose Sign in".into()),
        }
    }

    fn tooltip(&self) -> String {
        let mut t = match &self.usage {
            Some(u) if self.signed_in() => model::tooltip(u, chrono::Utc::now(), &chrono::Local),
            _ => String::new(),
        };
        if let Some(s) = self.status_line() {
            if !t.is_empty() {
                t.push('\n');
            }
            t.push_str(&s);
        }
        let t = if t.is_empty() { "claude-meter".to_string() } else { t };
        t.chars().take(127).collect()
    }

    fn badge_content(&self) -> badge::Content {
        let mut c = badge::Content { stale: matches!(self.status, Status::Failed(_)), ..Default::default() };
        match (&self.status, &self.usage) {
            (Status::SignedOut, _) => c.note = Some("Signed out".into()),
            (_, Some(u)) => {
                let mut push = |label: &str, w: &model::Window| c.rows.push((label.to_string(), w.percent, w.level()));
                if let Some(w) = &u.session {
                    push("Session", w);
                }
                if let Some(w) = &u.weekly {
                    push("Week", w);
                }
                for l in &u.others {
                    let label = l.label();
                    push(label.strip_prefix("Week · ").unwrap_or(&label), &l.window);
                }
            }
            (_, None) => c.note = Some("Loading…".into()),
        }
        c
    }

    fn build_menu(&self) -> Menu {
        let menu = Menu::new();
        let info = |text: &str| MenuItem::new(text, false, None);
        let now = chrono::Utc::now();
        if let (Some(u), true) = (&self.usage, self.signed_in()) {
            if let Some(w) = &u.session {
                let _ = menu.append(&info(&model::window_line("Session", w, now, &chrono::Local, false)));
            }
            if let Some(w) = &u.weekly {
                let _ = menu.append(&info(&model::window_line("Week", w, now, &chrono::Local, true)));
            }
            for l in &u.others {
                let _ = menu.append(&info(&model::window_line(&l.label(), &l.window, now, &chrono::Local, true)));
            }
            if let Some(line) = model::breakdown_line(&u.breakdown) {
                let _ = menu.append(&info(&line));
            }
        }
        if let Some(s) = self.status_line() {
            let _ = menu.append(&info(&s));
        }
        let sep = PredefinedMenuItem::separator();
        let _ = menu.append(&sep);
        let _ = menu.append(&MenuItem::with_id(id::REFRESH, "Refresh now", self.signed_in(), None));
        let _ = menu.append(&MenuItem::with_id(id::OPEN, "Open usage page", true, None));
        if self.signed_in() {
            let _ = menu.append(&MenuItem::with_id(id::SIGN_OUT, "Sign out", true, None));
        } else {
            let _ = menu.append(&MenuItem::with_id(id::SIGN_IN, "Sign in…", self.login.is_none(), None));
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&CheckMenuItem::with_id(id::BADGE, "Show badge", true, self.cfg.show_badge, None));
        let _ = menu.append(&CheckMenuItem::with_id(id::COMPACT, "Compact badge", self.cfg.show_badge, self.cfg.compact, None));
        let _ = menu.append(&CheckMenuItem::with_id(id::AUTOSTART, "Start with Windows", true, autostart::is_enabled(), None));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id(id::QUIT, "Quit", true, None));
        menu
    }

    /// Push the current state to the icon, tooltip, menu and badge.
    fn refresh_ui<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        let rgba = render::tray_icon(self.icon_state(), ICON_SIZE);
        if let Ok(icon) = Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE) {
            let _ = self.tray.set_icon(Some(icon));
        }
        let _ = self.tray.set_tooltip(Some(self.tooltip()));
        self.menu = self.build_menu();
        self.tray.set_menu(Some(Box::new(self.menu.clone())));
        self.sync_badge(target);
    }

    fn sync_badge<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        if !self.cfg.show_badge {
            if let Some(b) = self.badge.take() {
                self.remember_badge_position(&b);
            }
            return;
        }
        let content = self.badge_content();
        match &mut self.badge {
            Some(b) => b.update(content, self.cfg.compact),
            None => {
                let pos = self.cfg.badge_x.zip(self.cfg.badge_y).map(|(x, y)| PhysicalPosition::new(x, y));
                match Badge::open(target, pos, self.cfg.compact, content) {
                    Ok(b) => self.badge = Some(b),
                    Err(e) => eprintln!("badge: {e:#}"),
                }
            }
        }
    }

    fn remember_badge_position(&mut self, b: &Badge) {
        if let Some(p) = b.position() {
            if (self.cfg.badge_x, self.cfg.badge_y) != (Some(p.x), Some(p.y)) {
                self.cfg.badge_x = Some(p.x);
                self.cfg.badge_y = Some(p.y);
                self.cfg_dirty = Some(Instant::now() + Duration::from_secs(1));
            }
        }
    }

    fn open_login<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        if self.login.is_some() {
            return;
        }
        match LoginWindow::open(target, self.rejected_key.clone()) {
            Ok(w) => self.login = Some(w),
            Err(e) => eprintln!("{e:#}"),
        }
    }

    fn sign_out<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        secrets::clear();
        self.login = None;
        login::clear_profile();
        self.cfg.org_id = None;
        self.cfg.save();
        self.usage = None;
        self.rejected_key = None;
        self.status = Status::SignedOut;
        let _ = self.tx.send(Cmd::Key(None));
        self.refresh_ui(target);
    }

    fn on_menu<T: 'static>(&mut self, e: MenuEvent, target: &EventLoopWindowTarget<T>, flow: &mut ControlFlow) {
        match e.id.as_ref() {
            id::REFRESH => {
                let _ = self.tx.send(Cmd::Refresh);
            }
            id::OPEN => open_url(USAGE_URL),
            id::SIGN_IN => self.open_login(target),
            id::SIGN_OUT => self.sign_out(target),
            id::BADGE => {
                self.cfg.show_badge = !self.cfg.show_badge;
                self.cfg.save();
            }
            id::COMPACT => {
                self.cfg.compact = !self.cfg.compact;
                self.cfg.save();
            }
            id::AUTOSTART => {
                if let Err(e) = autostart::set(!autostart::is_enabled()) {
                    eprintln!("{e:#}");
                }
            }
            id::QUIT => {
                if let Some(b) = self.badge.take() {
                    self.remember_badge_position(&b);
                }
                *flow = ControlFlow::Exit;
                return;
            }
            _ => return,
        }
        self.refresh_ui(target);
    }

    fn on_fetched<T: 'static>(&mut self, f: Fetched, target: &EventLoopWindowTarget<T>) {
        match f {
            Fetched::Usage(u) => {
                self.usage = Some(u);
                self.status = Status::Ok;
            }
            Fetched::OrgId(o) => {
                self.cfg.org_id = Some(o);
                self.cfg.save();
                return;
            }
            Fetched::Error(e) if e.needs_login() => {
                // The worker already dropped the key. Keep it out of the sign-in window too.
                self.rejected_key = secrets::load();
                secrets::clear();
                self.status = Status::SignedOut;
            }
            Fetched::Error(e) => self.status = Status::Failed(e),
        }
        self.refresh_ui(target);
    }

    /// Called about once a second while the sign-in window is open.
    fn poll_login<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        let Some(key) = self.login.as_ref().and_then(|w| w.session_key()) else { return };
        self.login = None;
        if let Err(e) = secrets::save(&key) {
            eprintln!("warning: {e:#}");
        }
        self.rejected_key = None;
        self.status = Status::Loading;
        let _ = self.tx.send(Cmd::Key(Some(key)));
        self.refresh_ui(target);
    }
}

fn open_url(url: &str) {
    // explorer.exe hands URLs to the default browser without a console flash.
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
}

pub fn run() {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    {
        let p = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e| {
            let _ = p.send_event(UserEvent::Tray(e));
        }));
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = p.send_event(UserEvent::Menu(e));
        }));
    }

    let cfg = Config::load();
    let key = secrets::load();
    let (tx, rx) = mpsc::channel();
    {
        let (key, org, interval, proxy) = (key.clone(), cfg.org_id.clone(), cfg.interval(), proxy.clone());
        std::thread::Builder::new()
            .name("poll".into())
            .spawn(move || worker(proxy, rx, key, org, interval))
            .expect("starting the poll thread");
    }

    let menu = Menu::new();
    let tray = TrayIconBuilder::new()
        .with_tooltip("claude-meter")
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(true)
        .with_icon(Icon::from_rgba(render::tray_icon(IconState::Loading, ICON_SIZE), ICON_SIZE, ICON_SIZE).expect("icon"))
        .build()
        .expect("creating the tray icon");

    let status = if key.is_some() { Status::Loading } else { Status::SignedOut };
    let mut app = App {
        cfg,
        cfg_dirty: None,
        tx,
        tray,
        menu,
        usage: None,
        status,
        login: None,
        rejected_key: None,
        badge: None,
    };
    let mut first_run_login = key.is_none();

    use tao::platform::run_return::EventLoopExtRunReturn;
    event_loop.run_return(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(tao::event::StartCause::Init) => {
                app.refresh_ui(target);
                // First run: there's nothing to show until the person signs in.
                if std::mem::take(&mut first_run_login) {
                    app.open_login(target);
                    app.refresh_ui(target);
                }
            }
            Event::UserEvent(UserEvent::Menu(e)) => app.on_menu(e, target, flow),
            Event::UserEvent(UserEvent::Fetched(f)) => app.on_fetched(f, target),
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick { button: TrayButton::Left, .. })) => {
                open_url(USAGE_URL)
            }
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Enter { .. })) => {
                // Rebuild on hover so countdowns in the menu are current when it opens.
                app.menu = app.build_menu();
                app.tray.set_menu(Some(Box::new(app.menu.clone())));
            }
            Event::UserEvent(UserEvent::Tray(_)) => {}
            Event::WindowEvent { window_id, event, .. } => {
                let is_login = app.login.as_ref().is_some_and(|w| w.id() == window_id);
                let is_badge = app.badge.as_ref().is_some_and(|b| b.id() == window_id);
                match event {
                    WindowEvent::CloseRequested if is_login => {
                        app.login = None;
                        app.refresh_ui(target);
                    }
                    WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if is_badge => {
                        if let Some(b) = &app.badge {
                            b.drag();
                        }
                    }
                    WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right, .. } if is_badge => {
                        app.menu = app.build_menu();
                        if let Some(b) = &app.badge {
                            // SAFETY: the hwnd is the badge's live window, owned by this thread.
                            unsafe {
                                app.menu.show_context_menu_for_hwnd(b.hwnd(), None);
                            }
                        }
                    }
                    WindowEvent::Moved(_) if is_badge => {
                        if let Some(b) = app.badge.take() {
                            app.remember_badge_position(&b);
                            app.badge = Some(b);
                        }
                    }
                    WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Resized(_) if is_badge => {
                        if let Some(b) = &mut app.badge {
                            b.redraw();
                        }
                    }
                    _ => {}
                }
            }
            Event::RedrawRequested(window_id) => {
                if let Some(b) = app.badge.as_mut().filter(|b| b.id() == window_id) {
                    b.redraw();
                }
            }
            _ => {}
        }

        if app.login.is_some() {
            app.poll_login(target);
        }
        // Wake only for the sign-in check or a pending settings save; otherwise sleep.
        if *flow != ControlFlow::Exit {
            let now = Instant::now();
            if let Some(due) = app.cfg_dirty.filter(|d| *d <= now) {
                let _ = due;
                app.cfg.save();
                app.cfg_dirty = None;
            }
            let next_login = app.login.as_ref().map(|_| now + Duration::from_secs(1));
            if let Some(t) = [next_login, app.cfg_dirty].into_iter().flatten().min() {
                *flow = ControlFlow::WaitUntil(t);
            }
        } else if app.cfg_dirty.take().is_some() {
            app.cfg.save();
        }
    });
}
