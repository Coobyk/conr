//! TUI chrome palette. Content markup colors stay in `markup.rs`.
//!
//! `config.toml` `[theme]` sets six hex keys. Surfaces, hot/dim accents, and
//! selection colors are derived so a user only has to pick the documented set.

use std::cell::Cell;

use ratatui::style::Color;

use crate::config::Theme;

thread_local! {
    static CURRENT: Cell<Palette> = const { Cell::new(Palette::INK) };
}

/// Install the palette for this frame. Call once at the start of `draw`.
pub fn install(p: Palette) {
    CURRENT.set(p);
}

fn current() -> Palette {
    CURRENT.get()
}

pub fn bg() -> Color {
    current().bg
}
pub fn surface() -> Color {
    current().surface
}
pub fn surface2() -> Color {
    current().surface2
}
pub fn border() -> Color {
    current().border
}
pub fn text() -> Color {
    current().text
}
pub fn muted() -> Color {
    current().muted
}
pub fn purple() -> Color {
    current().accent
}
pub fn purple_dim() -> Color {
    current().accent_dim
}
pub fn purple_hot() -> Color {
    current().accent_hot
}
pub fn like() -> Color {
    current().like
}
pub fn rt() -> Color {
    current().rt
}
pub fn err() -> Color {
    current().error
}
pub fn ok() -> Color {
    current().success
}
pub fn select_bg() -> Color {
    current().select_bg
}
pub fn avatar_bg() -> Color {
    current().avatar_bg
}
pub fn bubble_me() -> Color {
    current().bubble_me
}
pub fn bubble_them() -> Color {
    current().bubble_them
}

/// First-ship spec defaults that older clients wrote into config.toml.
/// Those files never actually themed the TUI; treat them as "unset".
fn legacy_spec_theme() -> Theme {
    Theme {
        background: "#1D1F21".into(),
        foreground: "#C5C8C6".into(),
        accent: "#81A2BE".into(),
        error: "#CC6666".into(),
        success: "#B5BD68".into(),
        dim: "#5A5A5A".into(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub surface2: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_dim: Color,
    pub accent_hot: Color,
    pub like: Color,
    pub rt: Color,
    pub error: Color,
    pub success: Color,
    pub select_bg: Color,
    pub avatar_bg: Color,
    pub bubble_me: Color,
    pub bubble_them: Color,
}

impl Palette {
    /// Current shipped ink + violet chrome.
    pub const INK: Self = Self {
        bg: Color::Rgb(0x0a, 0x0a, 0x0f),
        surface: Color::Rgb(0x14, 0x14, 0x1c),
        surface2: Color::Rgb(0x1c, 0x1c, 0x28),
        border: Color::Rgb(0x2a, 0x2a, 0x3a),
        text: Color::Rgb(0xf0, 0xee, 0xf6),
        muted: Color::Rgb(0x8a, 0x86, 0x9a),
        accent: Color::Rgb(0xa7, 0x8b, 0xfa),
        accent_dim: Color::Rgb(0x5b, 0x45, 0x8f),
        accent_hot: Color::Rgb(0xc4, 0xb5, 0xfd),
        like: Color::Rgb(0xf4, 0x72, 0xb6),
        rt: Color::Rgb(0x34, 0xd3, 0x99),
        error: Color::Rgb(0xf8, 0x71, 0x71),
        success: Color::Rgb(0x34, 0xd3, 0x99),
        select_bg: Color::Rgb(0x1e, 0x1b, 0x2e),
        avatar_bg: Color::Rgb(0xa7, 0x8b, 0xfa),
        bubble_me: Color::Rgb(0x2e, 0x22, 0x4a),
        bubble_them: Color::Rgb(0x1a, 0x1a, 0x24),
    };

    pub fn from_theme(theme: &Theme) -> Self {
        if theme.is_legacy_spec() {
            return Self::INK;
        }
        let bg = parse_hex(&theme.background).unwrap_or(Self::INK.bg);
        let text = parse_hex(&theme.foreground).unwrap_or(Self::INK.text);
        let accent = parse_hex(&theme.accent).unwrap_or(Self::INK.accent);
        let error = parse_hex(&theme.error).unwrap_or(Self::INK.error);
        let success = parse_hex(&theme.success).unwrap_or(Self::INK.success);
        let muted = parse_hex(&theme.dim).unwrap_or(Self::INK.muted);

        let surface = mix(bg, text, 0.08);
        let surface2 = mix(bg, text, 0.14);
        let border = mix(bg, text, 0.22);
        let accent_dim = mix(bg, accent, 0.45);
        let accent_hot = mix(accent, Color::Rgb(0xff, 0xff, 0xff), 0.28);
        let select_bg = mix(bg, accent, 0.18);
        let bubble_me = mix(bg, accent, 0.32);
        let bubble_them = mix(bg, text, 0.10);
        let like = mix(accent, Color::Rgb(0xff, 0x80, 0xc0), 0.55);

        Self {
            bg,
            surface,
            surface2,
            border,
            text,
            muted,
            accent,
            accent_dim,
            accent_hot,
            like,
            rt: success,
            error,
            success,
            select_bg,
            avatar_bg: accent,
            bubble_me,
            bubble_them,
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::INK
    }
}

/// `#RGB` or `#RRGGBB`.
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim();
    let hex = s.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    Color::Rgb(
        lerp(ar, br, t),
        lerp(ag, bg, t),
        lerp(ab, bb, t),
    )
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rrggbb() {
        assert_eq!(parse_hex("#a78bfa"), Some(Color::Rgb(0xa7, 0x8b, 0xfa)));
        assert_eq!(parse_hex("  #0A0A0F "), Some(Color::Rgb(0x0a, 0x0a, 0x0f)));
    }

    #[test]
    fn parse_short() {
        assert_eq!(parse_hex("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn legacy_theme_keeps_ink() {
        let p = Palette::from_theme(&legacy_spec_theme());
        assert_eq!(p.accent, Palette::INK.accent);
        assert_eq!(p.bg, Palette::INK.bg);
    }

    #[test]
    fn custom_accent_applies() {
        let mut t = Theme::default();
        t.accent = "#ff0000".into();
        let p = Palette::from_theme(&t);
        assert_eq!(p.accent, Color::Rgb(255, 0, 0));
    }
}
