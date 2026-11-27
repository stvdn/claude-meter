//! The tray app: one event loop on the main thread, the poller on a worker thread.
//!
//! This module only draws and routes input. The rules live in `application`:
//! `Poller` decides when to fetch, `AppState` decides what a result means.
//! The worker blocks on HTTP and sleeps between polls, so the UI thread never waits
//! on the network and idles at ~0% CPU. The sign-in webview exists only while its
//! window is open.

use super::badge::{self, Badge};
use super::format;
use super::login_window::{self, LoginWindow};
use super::render::{self, IconState};
use crate::application::poller::{Command, Poller, Update};
use crate::application::ports::{Autostart, FetchError, SessionStore, SettingsStore, UsageSource};
use crate::application::state::{AppState, Status};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};
use tao::dpi::PhysicalPosition;
use tao::event::{ElementState, Event, MouseButton, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget};
use tray_icon::menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton as TrayButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

const USAGE_URL: &str = "https://claude.ai/settings/usage";
const ICON_SIZE: u32 = 32;

/// The adapters the tray app runs on, chosen by `main`.
pub struct Deps {
    pub source: Box<dyn UsageSource + Send>,
    pub sessions: Box<dyn SessionStore>,
    pub settings: Box<dyn SettingsStore>,
    pub autostart: Box<dyn Autostart>,
    /// Where the sign-in window keeps its browser profile.
    pub webview_profile: PathBuf,
}

#[derive(Debug)]
enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Update(Update),
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

struct Ui {
    state: AppState,
    poller: Sender<Command>,
    /// When to write the badge position after a drag (debounced).
    save_due: Option<Instant>,
    tray: TrayIcon,
    menu: Menu,
    login: Option<LoginWindow>,
    badge: Option<Badge>,
    webview_profile: PathBuf,
}

impl Ui {
    fn icon_state(&self) -> IconState {
        match (self.state.status(), self.state.usage()) {
            (Status::SignedOut, _) => IconState::SignedOut,
            (_, Some(u)) => IconState::from_usage(u, self.state.stale()),
            (_, None) => IconState::Loading,
        }
    }

    fn status_line(&self) -> Option<String> {
        match self.state.status() {
            Status::Loading => Some("Loading…".into()),
            Status::Ok => None,
            Status::Failed(FetchError::Blocked) => Some("⚠ claude.ai blocked the request · retrying".into()),
            Status::Failed(e) => Some(format!("⚠ Couldn't update ({e}) · retrying")),
            Status::SignedOut => Some("Signed out · choose Sign in".into()),
        }
    }

    fn tooltip(&self) -> String {
        let mut t = match self.state.usage() {
            Some(u) if self.state.signed_in() => format::tooltip(u, chrono::Utc::now(), &chrono::Local),
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
        let mut c = badge::Content { stale: self.state.stale(), ..Default::default() };
        match (self.state.status(), self.state.usage()) {
            (Status::SignedOut, _) => c.note = Some("Signed out".into()),
            (_, Some(u)) => {
                let mut push = |label: &str, w: &crate::domain::Window| c.rows.push((label.to_string(), w.percent, w.level()));
                if let Some(w) = &u.session {
                    push("Session", w);
                }
                if let Some(w) = &u.weekly {
                    push("Week", w);
                }
                for l in &u.others {
                    let label = format::limit_label(&l.kind);
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
        let signed_in = self.state.signed_in();
        if let (Some(u), true) = (self.state.usage(), signed_in) {
            if let Some(w) = &u.session {
                let _ = menu.append(&info(&format::window_line("Session", w, now, &chrono::Local, false)));
            }
            if let Some(w) = &u.weekly {
                let _ = menu.append(&info(&format::window_line("Week", w, now, &chrono::Local, true)));
            }
            for l in &u.others {
                let label = format::limit_label(&l.kind);
                let _ = menu.append(&info(&format::window_line(&label, &l.window, now, &chrono::Local, true)));
            }
            if let Some(line) = format::breakdown_line(&u.breakdown) {
                let _ = menu.append(&info(&line));
            }
        }
        if let Some(s) = self.status_line() {
            let _ = menu.append(&info(&s));
        }
        let settings = self.state.settings();
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id(id::REFRESH, "Refresh now", signed_in, None));
        let _ = menu.append(&MenuItem::with_id(id::OPEN, "Open usage page", true, None));
        if signed_in {
            let _ = menu.append(&MenuItem::with_id(id::SIGN_OUT, "Sign out", true, None));
        } else {
            let _ = menu.append(&MenuItem::with_id(id::SIGN_IN, "Sign in…", self.login.is_none(), None));
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&CheckMenuItem::with_id(id::BADGE, "Show badge", true, settings.show_badge, None));
        let _ = menu.append(&CheckMenuItem::with_id(id::COMPACT, "Compact badge", settings.show_badge, settings.compact, None));
        let _ = menu.append(&CheckMenuItem::with_id(id::AUTOSTART, "Start with Windows", true, self.state.autostart_enabled(), None));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id(id::QUIT, "Quit", true, None));
        menu
    }

    fn rebuild_menu(&mut self) {
        self.menu = self.build_menu();
        self.tray.set_menu(Some(Box::new(self.menu.clone())));
    }

    /// Push the current state to the icon, tooltip, menu and badge.
    fn refresh<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        let rgba = render::tray_icon(self.icon_state(), ICON_SIZE);
        if let Ok(icon) = Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE) {
            let _ = self.tray.set_icon(Some(icon));
        }
        let _ = self.tray.set_tooltip(Some(self.tooltip()));
        self.rebuild_menu();
        self.sync_badge(target);
    }

    fn sync_badge<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        let (show, compact) = (self.state.settings().show_badge, self.state.settings().compact);
        if !show {
            if let Some(b) = self.badge.take() {
                self.remember_badge_position(&b);
            }
            return;
        }
        let content = self.badge_content();
        match &mut self.badge {
            Some(b) => b.update(content, compact),
            None => {
                let s = self.state.settings();
                let pos = s.badge_x.zip(s.badge_y).map(|(x, y)| PhysicalPosition::new(x, y));
                match Badge::open(target, pos, compact, content) {
                    Ok(b) => self.badge = Some(b),
                    Err(e) => eprintln!("badge: {e:#}"),
                }
            }
        }
    }

    fn remember_badge_position(&mut self, b: &Badge) {
        if let Some(p) = b.position()
            && self.state.set_badge_position(p.x, p.y)
        {
            self.save_due = Some(Instant::now() + Duration::from_secs(1));
        }
    }

    fn open_login<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        if self.login.is_some() {
            return;
        }
        match LoginWindow::open(target, &self.webview_profile, self.state.rejected_session()) {
            Ok(w) => self.login = Some(w),
            Err(e) => eprintln!("{e:#}"),
        }
    }

    fn sign_out<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        self.login = None;
        login_window::clear_profile(&self.webview_profile);
        let _ = self.poller.send(self.state.sign_out());
        self.refresh(target);
    }

    fn on_menu<T: 'static>(&mut self, e: MenuEvent, target: &EventLoopWindowTarget<T>, flow: &mut ControlFlow) {
        match e.id.as_ref() {
            id::REFRESH => {
                let _ = self.poller.send(Command::Refresh);
            }
            id::OPEN => open_url(USAGE_URL),
            id::SIGN_IN => self.open_login(target),
            id::SIGN_OUT => self.sign_out(target),
            id::BADGE => self.state.toggle_badge(),
            id::COMPACT => self.state.toggle_compact(),
            id::AUTOSTART => {
                if let Err(e) = self.state.toggle_autostart() {
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
        self.refresh(target);
    }

    fn on_update<T: 'static>(&mut self, update: Update, target: &EventLoopWindowTarget<T>) {
        if self.state.apply(update) {
            self.refresh(target);
        }
    }

    /// Called about once a second while the sign-in window is open.
    fn poll_login<T: 'static>(&mut self, target: &EventLoopWindowTarget<T>) {
        let Some(session) = self.login.as_ref().and_then(|w| w.session_key()) else { return };
        self.login = None;
        let (command, saved) = self.state.sign_in(session);
        if let Err(e) = saved {
            eprintln!("warning: {e:#}");
        }
        let _ = self.poller.send(command);
        self.refresh(target);
    }
}

fn open_url(url: &str) {
    // explorer.exe hands URLs to the default browser without a console flash.
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
}

pub fn run(deps: Deps) {
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

    let state = AppState::new(deps.sessions, deps.settings, deps.autostart);
    let session = state.session();
    let (tx, rx) = mpsc::channel();
    {
        let poller = Poller::new(deps.source, session.clone(), state.settings().org_id.clone(), state.settings().interval());
        let proxy = proxy.clone();
        std::thread::Builder::new()
            .name("poll".into())
            .spawn(move || poller.run(rx, |u| proxy.send_event(UserEvent::Update(u)).is_ok()))
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

    let mut ui = Ui {
        state,
        poller: tx,
        save_due: None,
        tray,
        menu,
        login: None,
        badge: None,
        webview_profile: deps.webview_profile,
    };
    let mut first_run_login = session.is_none();

    use tao::platform::run_return::EventLoopExtRunReturn;
    event_loop.run_return(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(tao::event::StartCause::Init) => {
                ui.refresh(target);
                // First run: there's nothing to show until the person signs in.
                if std::mem::take(&mut first_run_login) {
                    ui.open_login(target);
                    ui.refresh(target);
                }
            }
            Event::UserEvent(UserEvent::Menu(e)) => ui.on_menu(e, target, flow),
            Event::UserEvent(UserEvent::Update(u)) => ui.on_update(u, target),
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::DoubleClick { button: TrayButton::Left, .. })) => {
                open_url(USAGE_URL)
            }
            // Rebuild on hover so countdowns in the menu are current when it opens.
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Enter { .. })) => ui.rebuild_menu(),
            Event::UserEvent(UserEvent::Tray(_)) => {}
            Event::WindowEvent { window_id, event, .. } => {
                let is_login = ui.login.as_ref().is_some_and(|w| w.id() == window_id);
                let is_badge = ui.badge.as_ref().is_some_and(|b| b.id() == window_id);
                match event {
                    WindowEvent::CloseRequested if is_login => {
                        ui.login = None;
                        ui.refresh(target);
                    }
                    WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if is_badge => {
                        if let Some(b) = &ui.badge {
                            b.drag();
                        }
                    }
                    WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right, .. } if is_badge => {
                        ui.menu = ui.build_menu();
                        if let Some(b) = &ui.badge {
                            // SAFETY: the hwnd is the badge's live window, owned by this thread.
                            unsafe {
                                ui.menu.show_context_menu_for_hwnd(b.hwnd(), None);
                            }
                        }
                    }
                    WindowEvent::Moved(_) if is_badge => {
                        if let Some(b) = ui.badge.take() {
                            ui.remember_badge_position(&b);
                            ui.badge = Some(b);
                        }
                    }
                    WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Resized(_) if is_badge => {
                        if let Some(b) = &mut ui.badge {
                            b.redraw();
                        }
                    }
                    _ => {}
                }
            }
            Event::RedrawRequested(window_id) => {
                if let Some(b) = ui.badge.as_mut().filter(|b| b.id() == window_id) {
                    b.redraw();
                }
            }
            _ => {}
        }

        if ui.login.is_some() {
            ui.poll_login(target);
        }
        // Wake only for the sign-in check or a pending settings save; otherwise sleep.
        let now = Instant::now();
        let quitting = *flow == ControlFlow::Exit;
        if (quitting || ui.save_due.is_some_and(|d| d <= now)) && ui.save_due.take().is_some() {
            ui.state.save_settings();
        }
        if *flow != ControlFlow::Exit {
            let next_login = ui.login.as_ref().map(|_| now + Duration::from_secs(1));
            if let Some(t) = [next_login, ui.save_due].into_iter().flatten().min() {
                *flow = ControlFlow::WaitUntil(t);
            }
        }
    });
}
