//! The floating badge: a small always-on-top window with the session and weekly bars.
//! Drag it anywhere with the left button; right-click opens the same menu as the tray icon.

use crate::render::{self, BadgeRow};
use anyhow::{Context, Result, anyhow};
use softbuffer::Surface;
use std::num::NonZeroU32;
use std::rc::Rc;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event_loop::EventLoopWindowTarget;
use tao::platform::windows::WindowBuilderExtWindows;
use tao::window::{Window, WindowBuilder, WindowId};

/// What the badge shows; owned so it can be redrawn on demand (DPI changes, expose).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Content {
    pub rows: Vec<(String, f32, crate::model::Level)>,
    pub stale: bool,
    pub note: Option<String>,
}

pub struct Badge {
    // Drop order: the surface before its window.
    surface: Surface<Rc<Window>, Rc<Window>>,
    window: Rc<Window>,
    compact: bool,
    content: Content,
}

impl Badge {
    pub fn open<T: 'static>(
        target: &EventLoopWindowTarget<T>,
        position: Option<PhysicalPosition<i32>>,
        compact: bool,
        content: Content,
    ) -> Result<Self> {
        let (w, h) = render::badge_size(compact, content.rows.len());
        let window = WindowBuilder::new()
            .with_title("claude-meter")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_inner_size(LogicalSize::new(w, h))
            .with_visible(false)
            .build(target)
            .context("creating the badge window")?;
        let window = Rc::new(window);
        let pos = position
            .filter(|p| on_some_monitor(&window, *p))
            .unwrap_or_else(|| default_position(&window));
        window.set_outer_position(pos);
        let context = softbuffer::Context::new(window.clone()).map_err(|e| anyhow!("{e}"))?;
        let surface = Surface::new(&context, window.clone()).map_err(|e| anyhow!("{e}"))?;
        let mut badge = Self { surface, window, compact, content };
        badge.redraw();
        badge.window.set_visible(true);
        Ok(badge)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn hwnd(&self) -> isize {
        use tao::platform::windows::WindowExtWindows;
        self.window.hwnd() as isize
    }

    pub fn position(&self) -> Option<PhysicalPosition<i32>> {
        self.window.outer_position().ok()
    }

    pub fn drag(&self) {
        let _ = self.window.drag_window();
    }

    /// New numbers and/or layout. Resizes when the number of rows or the mode changes.
    pub fn update(&mut self, content: Content, compact: bool) {
        if content == self.content && compact == self.compact {
            return;
        }
        let resize = compact != self.compact || content.rows.len() != self.content.rows.len();
        self.content = content;
        self.compact = compact;
        if resize {
            let (w, h) = render::badge_size(compact, self.content.rows.len());
            self.window.set_inner_size(LogicalSize::new(w, h));
        }
        self.redraw();
    }

    pub fn redraw(&mut self) {
        let size = self.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if self.surface.resize(w, h).is_err() {
            return;
        }
        let rows: Vec<BadgeRow> =
            self.content.rows.iter().map(|(label, percent, level)| BadgeRow { label, percent: *percent, level: *level }).collect();
        let pixels = render::badge(
            w.get(),
            h.get(),
            self.window.scale_factor() as f32,
            &rows,
            self.compact,
            self.content.stale,
            self.content.note.as_deref(),
        );
        if let Ok(mut buf) = self.surface.buffer_mut() {
            let n = buf.len().min(pixels.len());
            buf[..n].copy_from_slice(&pixels[..n]);
            let _ = buf.present();
        }
    }
}

fn on_some_monitor(window: &Window, p: PhysicalPosition<i32>) -> bool {
    window.available_monitors().any(|m| {
        let (mp, ms) = (m.position(), m.size());
        p.x >= mp.x && p.y >= mp.y && p.x < mp.x + ms.width as i32 - 20 && p.y < mp.y + ms.height as i32 - 20
    })
}

/// Bottom-right of the primary monitor, clear of a normal-height taskbar.
fn default_position(window: &Window) -> PhysicalPosition<i32> {
    let size = window.outer_size();
    match window.primary_monitor().or_else(|| window.current_monitor()) {
        Some(m) => {
            let s = m.scale_factor();
            let (mp, ms) = (m.position(), m.size());
            PhysicalPosition::new(
                mp.x + ms.width as i32 - size.width as i32 - (16.0 * s) as i32,
                mp.y + ms.height as i32 - size.height as i32 - (64.0 * s) as i32,
            )
        }
        None => PhysicalPosition::new(100, 100),
    }
}
