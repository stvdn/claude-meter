//! Drawing for the tray icon and the floating badge, in plain RGBA pixel buffers.
//!
//! Text uses a font that ships with Windows (Segoe UI), read from disk once, so the
//! binary carries no font. If no font can be loaded the pictures still draw, without text.

use crate::model::{Level, Usage};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

/// What the tray icon should say right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IconState {
    /// Fresh numbers: session percent on a background colored by the worst limit,
    /// with the weekly percent as a bar along the bottom.
    Usage { session: f32, weekly: Option<f32>, level: Level },
    /// Last known numbers, but the latest update failed: same picture in gray.
    Stale { session: f32, weekly: Option<f32> },
    /// Still loading.
    Loading,
    /// Signed out: needs the sign-in window.
    SignedOut,
}

impl IconState {
    pub fn from_usage(u: &Usage, stale: bool) -> Self {
        let session = u.session.as_ref().map_or(0.0, |w| w.percent);
        let weekly = u.weekly.as_ref().map(|w| w.percent);
        if stale { IconState::Stale { session, weekly } } else { IconState::Usage { session, weekly, level: u.level() } }
    }
}

pub struct Rgb(pub u8, pub u8, pub u8);

pub fn level_color(level: Level) -> Rgb {
    match level {
        Level::Ok => Rgb(0x2f, 0x8f, 0x5b),
        Level::Warn => Rgb(0xd0, 0x8a, 0x16),
        Level::Critical => Rgb(0xd0, 0x45, 0x3a),
    }
}

const GRAY: Rgb = Rgb(0x6b, 0x6b, 0x6b);
const WHITE: Rgb = Rgb(0xff, 0xff, 0xff);

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        ["seguisb.ttf", "segoeuib.ttf", "segoeui.ttf", "arialbd.ttf", "arial.ttf"]
            .iter()
            .find_map(|f| std::fs::read(dir.join("Fonts").join(f)).ok().and_then(|b| FontVec::try_from_vec(b).ok()))
    })
    .as_ref()
}

fn paint(c: &Rgb, alpha: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c.0, c.1, c.2, alpha));
    p.anti_alias = true;
    p
}

fn rounded_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, color: &Rgb, alpha: u8) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint(color, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

fn rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: &Rgb, alpha: u8) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(color, alpha), Transform::identity(), None);
    }
}

/// Width in pixels of `text` at `size` px.
fn text_width(text: &str, size: f32) -> f32 {
    let Some(f) = font() else { return 0.0 };
    let sf = f.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Draw `text` with its baseline-box top-left at (x, y): y is the top of the ascent.
fn draw_text(pm: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: &Rgb) {
    let Some(f) = font() else { return };
    let sf = f.as_scaled(PxScale::from(size));
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let baseline = y + sf.ascent();
    let mut caret = x;
    let mut prev = None;
    let data = pm.data_mut();
    for c in text.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), point(caret, baseline));
        caret += sf.h_advance(id);
        prev = Some(id);
        let Some(outline) = f.outline_glyph(glyph) else { continue };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, cov| {
            let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
            if px < 0 || py < 0 || px >= w || py >= h || cov <= 0.0 {
                return;
            }
            // Source-over blend onto premultiplied RGBA.
            let i = ((py * w + px) * 4) as usize;
            let a = cov.min(1.0);
            let src = [color.0 as f32 * a, color.1 as f32 * a, color.2 as f32 * a, 255.0 * a];
            for k in 0..4 {
                data[i + k] = (src[k] + data[i + k] as f32 * (1.0 - a)).round().min(255.0) as u8;
            }
        });
    }
}

/// Size that makes `text` fit in `max_w`, starting from `size`.
fn fit(text: &str, size: f32, max_w: f32) -> f32 {
    let w = text_width(text, size);
    if w > max_w && w > 0.0 { size * max_w / w } else { size }
}

/// Straight (not premultiplied) RGBA bytes, which is what tray icons take.
fn to_rgba(pm: &Pixmap) -> Vec<u8> {
    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// The tray icon as `size`×`size` straight RGBA (draw at 32 so it stays sharp at 200 % scaling).
pub fn tray_icon(state: IconState, size: u32) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("icon size is non-zero");
    let s = size as f32;
    let (bg, label, weekly) = match state {
        IconState::Usage { session, weekly, level } => (level_color(level), number(session), weekly),
        IconState::Stale { session, weekly } => (GRAY, number(session), weekly),
        IconState::Loading => (GRAY, "…".to_string(), None),
        IconState::SignedOut => (GRAY, "?".to_string(), None),
    };
    rounded_rect(&mut pm, 0.0, 0.0, s, s, s * 0.22, &bg, 255);

    // Weekly bar along the bottom: a faint track and a white fill.
    let bar_h = (s * 0.16).round().max(2.0);
    let text_h = if weekly.is_some() { s - bar_h - s * 0.06 } else { s };
    if let Some(w) = weekly {
        let (x, y, bw) = (s * 0.14, s - bar_h - s * 0.08, s * 0.72);
        rect(&mut pm, x, y, bw, bar_h, &WHITE, 90);
        rect(&mut pm, x, y, bw * (w.clamp(0.0, 100.0) / 100.0), bar_h, &WHITE, 255);
    }

    let size_px = fit(&label, s * 0.72, s * 0.92);
    if let Some(f) = font() {
        let sf = f.as_scaled(PxScale::from(size_px));
        let cap = sf.ascent() * 0.72; // digits are about cap height
        let x = (s - text_width(&label, size_px)) / 2.0;
        let y = (text_h - cap) / 2.0 - (sf.ascent() - cap);
        draw_text(&mut pm, &label, x, y, size_px, &WHITE);
    }
    to_rgba(&pm)
}

/// `42.4` -> `"42"`; 100 and over stay three digits, the icon shrinks the text to fit.
fn number(percent: f32) -> String {
    format!("{}", percent.clamp(0.0, 999.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_has_the_right_size_and_is_opaque_in_the_middle() {
        for state in [
            IconState::Usage { session: 42.0, weekly: Some(18.0), level: Level::Ok },
            IconState::Stale { session: 100.0, weekly: None },
            IconState::Loading,
            IconState::SignedOut,
        ] {
            let px = tray_icon(state, 32);
            assert_eq!(px.len(), 32 * 32 * 4);
            assert_eq!(px[(16 * 32 + 16) * 4 + 3], 255);
        }
    }

    #[test]
    fn icon_background_follows_the_level() {
        let px = tray_icon(IconState::Usage { session: 90.0, weekly: None, level: Level::Critical }, 32);
        let c = level_color(Level::Critical);
        // A pixel near the corner but inside the rounding, away from the digits.
        let i = (8 * 32 + 2) * 4;
        assert_eq!((px[i], px[i + 1], px[i + 2]), (c.0, c.1, c.2));
    }

    #[test]
    fn numbers() {
        assert_eq!(number(42.4), "42");
        assert_eq!(number(-1.0), "0");
        assert_eq!(number(100.0), "100");
    }
}

