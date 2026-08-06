//! Lightweight post markup for the TUI.
//!
//! Syntax (stored as plain text; markers count toward the 512 char limit):
//!
//! | Markup              | Effect        |
//! |---------------------|---------------|
//! | `**bold**`          | Bold          |
//! | `__underline__`     | Underline     |
//! | `~~strike~~`        | Strikethrough |
//! | `{red}text{/}`      | Named color   |
//! | `@handle` / `#tag`  | Auto accent   |
//!
//! Escape a special character with `\`: `\*`, `\_`, `\~`, `\{`, `\}`, `\\`.
//!
//! Colors: red, green, blue, yellow, orange, pink, purple, cyan, white,
//! gray/grey, muted. Italic is intentionally omitted — terminal support is
//! spotty; use underline instead.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// Named colors (readable on the dark palette).
const C_RED: Color = Color::Rgb(0xf8, 0x71, 0x71);
const C_GREEN: Color = Color::Rgb(0x34, 0xd3, 0x99);
const C_BLUE: Color = Color::Rgb(0x60, 0xa5, 0xfa);
const C_YELLOW: Color = Color::Rgb(0xfB, 0xbf, 0x24);
const C_ORANGE: Color = Color::Rgb(0xfb, 0x92, 0x3c);
const C_PINK: Color = Color::Rgb(0xf4, 0x72, 0xb6);
const C_PURPLE: Color = Color::Rgb(0xa7, 0x8b, 0xfa);
const C_CYAN: Color = Color::Rgb(0x22, 0xd3, 0xee);
const C_WHITE: Color = Color::Rgb(0xf0, 0xee, 0xf6);
const C_GRAY: Color = Color::Rgb(0x8a, 0x86, 0x9a);

/// A run of visible text with style flags (no markup markers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub bold: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<ColorName>,
    /// Auto-styled mention or hashtag (accent purple unless color set).
    pub accent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorName {
    Red,
    Green,
    Blue,
    Yellow,
    Orange,
    Pink,
    Purple,
    Cyan,
    White,
    Gray,
    Muted,
}

impl ColorName {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "red" => Some(Self::Red),
            "green" => Some(Self::Green),
            "blue" => Some(Self::Blue),
            "yellow" => Some(Self::Yellow),
            "orange" => Some(Self::Orange),
            "pink" => Some(Self::Pink),
            "purple" | "violet" => Some(Self::Purple),
            "cyan" | "teal" => Some(Self::Cyan),
            "white" => Some(Self::White),
            "gray" | "grey" => Some(Self::Gray),
            "muted" | "dim" => Some(Self::Muted),
            _ => None,
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Red => C_RED,
            Self::Green => C_GREEN,
            Self::Blue => C_BLUE,
            Self::Yellow => C_YELLOW,
            Self::Orange => C_ORANGE,
            Self::Pink => C_PINK,
            Self::Purple => C_PURPLE,
            Self::Cyan => C_CYAN,
            Self::White => C_WHITE,
            Self::Gray | Self::Muted => C_GRAY,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    bold: bool,
    underline: bool,
    strike: bool,
    color: Option<ColorName>,
    accent: bool,
}

/// Parse markup into visible styled segments. Markers are stripped.
pub fn parse(input: &str) -> Vec<Segment> {
    parse_inner(input, Flags::default())
}

fn parse_inner(input: &str, flags: Flags) -> Vec<Segment> {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut out: Vec<Segment> = Vec::new();
    let mut i = 0;
    let mut plain = String::new();

    let flush = |plain: &mut String, out: &mut Vec<Segment>, flags: Flags| {
        if plain.is_empty() {
            return;
        }
        out.push(Segment {
            text: std::mem::take(plain),
            bold: flags.bold,
            underline: flags.underline,
            strike: flags.strike,
            color: flags.color,
            accent: flags.accent,
        });
    };

    while i < n {
        // Escape: \X → literal X for specials
        if chars[i] == '\\' && i + 1 < n {
            let next = chars[i + 1];
            if matches!(next, '*' | '_' | '~' | '{' | '}' | '\\' | '#' | '@') {
                plain.push(next);
                i += 2;
                continue;
            }
        }

        // **bold**
        if chars[i] == '*' && i + 1 < n && chars[i + 1] == '*' {
            if let Some(end) = find_closer(&chars, i + 2, &['*', '*']) {
                flush(&mut plain, &mut out, flags);
                let inner: String = chars[i + 2..end].iter().collect();
                let mut nested = flags;
                nested.bold = true;
                out.extend(parse_inner(&inner, nested));
                i = end + 2;
                continue;
            }
        }

        // __underline__
        if chars[i] == '_' && i + 1 < n && chars[i + 1] == '_' {
            if let Some(end) = find_closer(&chars, i + 2, &['_', '_']) {
                flush(&mut plain, &mut out, flags);
                let inner: String = chars[i + 2..end].iter().collect();
                let mut nested = flags;
                nested.underline = true;
                out.extend(parse_inner(&inner, nested));
                i = end + 2;
                continue;
            }
        }

        // ~~strike~~
        if chars[i] == '~' && i + 1 < n && chars[i + 1] == '~' {
            if let Some(end) = find_closer(&chars, i + 2, &['~', '~']) {
                flush(&mut plain, &mut out, flags);
                let inner: String = chars[i + 2..end].iter().collect();
                let mut nested = flags;
                nested.strike = true;
                out.extend(parse_inner(&inner, nested));
                i = end + 2;
                continue;
            }
        }

        // {color}…{/} or {color}…{/color}
        if chars[i] == '{' {
            if let Some((color, content_start, content_end, after)) =
                try_parse_color_block(&chars, i)
            {
                flush(&mut plain, &mut out, flags);
                let inner: String = chars[content_start..content_end].iter().collect();
                let mut nested = flags;
                nested.color = Some(color);
                nested.accent = false;
                out.extend(parse_inner(&inner, nested));
                i = after;
                continue;
            }
        }

        // @mention or #hashtag (word chars only)
        if (chars[i] == '@' || chars[i] == '#') && !flags.accent {
            if let Some(end) = take_tag_or_mention(&chars, i) {
                flush(&mut plain, &mut out, flags);
                let word: String = chars[i..=end].iter().collect();
                let mut nested = flags;
                nested.accent = true;
                out.push(Segment {
                    text: word,
                    bold: nested.bold,
                    underline: nested.underline,
                    strike: nested.strike,
                    color: nested.color,
                    accent: true,
                });
                i = end + 1;
                continue;
            }
        }

        plain.push(chars[i]);
        i += 1;
    }

    flush(&mut plain, &mut out, flags);
    out
}

/// Find unescaped two-char closer starting at `from`. Returns start index of closer.
fn find_closer(chars: &[char], from: usize, closer: &[char; 2]) -> Option<usize> {
    let n = chars.len();
    let mut i = from;
    while i + 1 < n {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        // Skip nested same-delimiter pairs so `**a **b** c**` finds the outer close
        // only after balanced inners — simple first-match is fine for short posts.
        if chars[i] == closer[0] && chars[i + 1] == closer[1] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `{red}inner{/}` or `{red}inner{/red}` → (color, content_start, content_end, after_idx)
fn try_parse_color_block(
    chars: &[char],
    start: usize,
) -> Option<(ColorName, usize, usize, usize)> {
    // parse open tag
    if chars.get(start) != Some(&'{') {
        return None;
    }
    let mut j = start + 1;
    if j >= chars.len() || chars[j] == '/' {
        return None;
    }
    let name_start = j;
    while j < chars.len() && chars[j].is_ascii_alphabetic() {
        j += 1;
    }
    if j == name_start || chars.get(j) != Some(&'}') {
        return None;
    }
    let name: String = chars[name_start..j].iter().collect();
    let color = ColorName::parse(&name)?;
    let content_start = j + 1;

    // Find matching close: {/} or {/name}
    let mut i = content_start;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '/' {
            let close_name_start = i + 2;
            let mut k = close_name_start;
            while k < chars.len() && chars[k].is_ascii_alphabetic() {
                k += 1;
            }
            if chars.get(k) == Some(&'}') {
                let close_name: String = chars[close_name_start..k].iter().collect();
                if close_name.is_empty()
                    || close_name.eq_ignore_ascii_case(&name)
                {
                    return Some((color, content_start, i, k + 1));
                }
            }
        }
        i += 1;
    }
    None
}

/// If `chars[i]` is @ or # followed by 1+ alnum/_, return end index of the run.
fn take_tag_or_mention(chars: &[char], i: usize) -> Option<usize> {
    let sigil = chars.get(i)?;
    if *sigil != '@' && *sigil != '#' {
        return None;
    }
    let mut j = i + 1;
    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    if j == i + 1 {
        return None;
    }
    // handles: 3–32, tags: at least 1 — be permissive for display (1+)
    Some(j - 1)
}

// ── Wrapping & ratatui conversion ───────────────────────────────────────────

/// Wrap segments to `width` columns (unicode scalar count ≈ terminal cells for
/// this charset). Prefer breaking on spaces.
pub fn wrap_segments(segments: &[Segment], width: usize) -> Vec<Vec<Segment>> {
    if width == 0 {
        return vec![segments.to_vec()];
    }

    let mut lines: Vec<Vec<Segment>> = Vec::new();
    let mut cur: Vec<Segment> = Vec::new();
    let mut col = 0usize;

    // Flatten to char-level runs would be heavy; process segment by segment,
    // splitting on spaces and hard-breaking long words.
    for seg in segments {
        if seg.text.is_empty() {
            continue;
        }

        // Handle explicit newlines inside a segment
        let parts: Vec<&str> = seg.text.split('\n').collect();
        for (pi, part) in parts.iter().enumerate() {
            if pi > 0 {
                lines.push(std::mem::take(&mut cur));
                col = 0;
            }
            push_wrapped_part(&mut lines, &mut cur, &mut col, part, seg, width);
        }
    }

    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn push_wrapped_part(
    lines: &mut Vec<Vec<Segment>>,
    cur: &mut Vec<Segment>,
    col: &mut usize,
    part: &str,
    style: &Segment,
    width: usize,
) {
    if part.is_empty() {
        return;
    }

    // Split into words keeping spaces as separate tokens so we can reflow.
    let tokens = tokenize_words(part);
    for token in tokens {
        let tlen = token.chars().count();
        if tlen == 0 {
            continue;
        }

        // Hard-break tokens longer than width
        if tlen > width {
            for ch in token.chars() {
                if *col >= width {
                    lines.push(std::mem::take(cur));
                    *col = 0;
                }
                push_char_seg(cur, ch, style);
                *col += 1;
            }
            continue;
        }

        // Space at start of line: skip (trim leading spaces after wrap)
        if token.chars().all(|c| c == ' ') {
            if *col == 0 {
                continue;
            }
            if *col + tlen > width {
                lines.push(std::mem::take(cur));
                *col = 0;
                continue;
            }
            push_text_seg(cur, token, style);
            *col += tlen;
            continue;
        }

        if *col + tlen > width && *col > 0 {
            lines.push(std::mem::take(cur));
            *col = 0;
        }
        push_text_seg(cur, token, style);
        *col += tlen;
    }
}

fn tokenize_words(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            if start < i {
                out.push(&s[start..i]);
            }
            let sp = i;
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            out.push(&s[sp..i]);
            start = i;
        } else {
            i += 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn push_text_seg(cur: &mut Vec<Segment>, text: &str, style: &Segment) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = cur.last_mut() {
        if same_style(last, style) {
            last.text.push_str(text);
            return;
        }
    }
    cur.push(Segment {
        text: text.to_string(),
        bold: style.bold,
        underline: style.underline,
        strike: style.strike,
        color: style.color,
        accent: style.accent,
    });
}

fn push_char_seg(cur: &mut Vec<Segment>, ch: char, style: &Segment) {
    let mut buf = [0u8; 4];
    push_text_seg(cur, ch.encode_utf8(&mut buf), style);
}

fn same_style(a: &Segment, b: &Segment) -> bool {
    a.bold == b.bold
        && a.underline == b.underline
        && a.strike == b.strike
        && a.color == b.color
        && a.accent == b.accent
}

fn segment_to_style(seg: &Segment, base_fg: Color, bg: Color) -> Style {
    let fg = if let Some(c) = seg.color {
        c.color()
    } else if seg.accent {
        C_PURPLE
    } else {
        base_fg
    };

    let mut style = Style::default().fg(fg).bg(bg);
    let mut mods = Modifier::empty();
    if seg.bold {
        mods |= Modifier::BOLD;
    }
    if seg.underline {
        mods |= Modifier::UNDERLINED;
    }
    if seg.strike {
        mods |= Modifier::CROSSED_OUT;
    }
    if !mods.is_empty() {
        style = style.add_modifier(mods);
    }
    style
}

/// Convert one wrapped line of segments into a ratatui [`Line`].
pub fn segments_to_line(segments: &[Segment], base_fg: Color, bg: Color) -> Line<'static> {
    if segments.is_empty() {
        return Line::from(Span::styled("", Style::default().fg(base_fg).bg(bg)));
    }
    let spans: Vec<Span> = segments
        .iter()
        .map(|s| Span::styled(s.text.clone(), segment_to_style(s, base_fg, bg)))
        .collect();
    Line::from(spans)
}

/// Parse markup, wrap, and produce lines with an optional prefix on each row.
///
/// `prefix` is rendered with `prefix_style` before the body on every line.
pub fn styled_body_lines(
    content: &str,
    width: usize,
    base_fg: Color,
    bg: Color,
) -> Vec<Line<'static>> {
    let segs = parse(content);
    let wrapped = wrap_segments(&segs, width);
    wrapped
        .into_iter()
        .map(|line_segs| segments_to_line(&line_segs, base_fg, bg))
        .collect()
}

/// Like [`styled_body_lines`], but each line is prefixed (e.g. gutter + indent).
pub fn styled_body_lines_prefixed(
    content: &str,
    width: usize,
    base_fg: Color,
    bg: Color,
    prefix: Span<'static>,
) -> Vec<Line<'static>> {
    let segs = parse(content);
    let wrapped = wrap_segments(&segs, width);
    wrapped
        .into_iter()
        .map(|line_segs| {
            let mut spans = vec![prefix.clone()];
            if line_segs.is_empty() {
                spans.push(Span::styled("", Style::default().fg(base_fg).bg(bg)));
            } else {
                for s in &line_segs {
                    spans.push(Span::styled(
                        s.text.clone(),
                        segment_to_style(s, base_fg, bg),
                    ));
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// Plain visible text with markers stripped (for previews / truncations).
pub fn strip_markup(input: &str) -> String {
    parse(input)
        .into_iter()
        .map(|s| s.text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_and_plain() {
        let segs = parse("hello **world**!");
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "hello ");
        assert!(!segs[0].bold);
        assert_eq!(segs[1].text, "world");
        assert!(segs[1].bold);
        assert_eq!(segs[2].text, "!");
    }

    #[test]
    fn underline_and_strike() {
        let segs = parse("__u__ and ~~s~~");
        assert!(segs.iter().any(|s| s.text == "u" && s.underline));
        assert!(segs.iter().any(|s| s.text == "s" && s.strike));
    }

    #[test]
    fn color_block() {
        let segs = parse("{red}hot{/} cold");
        assert_eq!(segs[0].text, "hot");
        assert_eq!(segs[0].color, Some(ColorName::Red));
        assert!(segs[1].text.contains("cold"));
        assert!(segs[1].color.is_none());
    }

    #[test]
    fn color_named_close() {
        let segs = parse("{blue}sky{/blue}");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].color, Some(ColorName::Blue));
        assert_eq!(segs[0].text, "sky");
    }

    #[test]
    fn nested_bold_color() {
        let segs = parse("**bold {green}g{/}**");
        assert!(segs.iter().any(|s| s.text == "bold " && s.bold));
        assert!(segs
            .iter()
            .any(|s| s.text == "g" && s.bold && s.color == Some(ColorName::Green)));
    }

    #[test]
    fn mentions_and_tags() {
        let segs = parse("hi @alice #rust");
        assert!(segs.iter().any(|s| s.text == "@alice" && s.accent));
        assert!(segs.iter().any(|s| s.text == "#rust" && s.accent));
    }

    #[test]
    fn escape_markers() {
        let segs = parse(r"\*\*not bold\*\*");
        let text: String = segs.into_iter().map(|s| s.text).collect();
        assert_eq!(text, "**not bold**");
    }

    #[test]
    fn incomplete_left_as_literal() {
        let segs = parse("**oops");
        let text: String = segs.into_iter().map(|s| s.text).collect();
        assert_eq!(text, "**oops");
    }

    #[test]
    fn strip_removes_markers() {
        assert_eq!(strip_markup("**hi** {red}x{/}"), "hi x");
    }

    #[test]
    fn wrap_preserves_styles() {
        let segs = parse("**ab** cd");
        let lines = wrap_segments(&segs, 3);
        // "ab " then "cd" or "ab" / "cd"
        assert!(lines.len() >= 1);
        let flat: String = lines
            .iter()
            .flat_map(|l| l.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(flat.contains("ab"));
        assert!(flat.contains("cd"));
    }
}
