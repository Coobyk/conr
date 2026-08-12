//! Composer helpers: markup wrap shortcuts, cursor offsets, color autocomplete apply.

use tui_textarea::{CursorMove, TextArea};

use super::markup::{self, ColorQuery};

/// Character offset of the cursor in `lines().join("\n")` space.
pub fn cursor_char_offset(ta: &TextArea<'_>) -> usize {
    let (row, col) = ta.cursor();
    let lines = ta.lines();
    let mut off = 0usize;
    for r in 0..row {
        off += lines.get(r).map(|l| l.chars().count()).unwrap_or(0) + 1;
    }
    off + col
}

/// Map a character offset in joined content back to `(row, col)`.
pub fn char_to_rowcol(lines: &[String], char_off: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (0, 0);
    }
    let mut rem = char_off;
    for (r, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        if rem <= len {
            return (r, rem);
        }
        rem = rem.saturating_sub(len + 1); // +1 for newline between lines
        if r + 1 == lines.len() {
            return (r, len);
        }
    }
    let last = lines.len() - 1;
    (last, lines[last].chars().count())
}

/// Wrap the selection (or insert empty markers) with `open`/`close`.
///
/// If the selection is already wrapped with the same markers, unwraps (toggle).
pub fn wrap_markup(ta: &mut TextArea<'_>, open: &str, close: &str) {
    if ta.selection_range().is_some() {
        ta.cut();
        let selected = ta.yank_text();
        let replacement = toggle_wrap(&selected, open, close);
        ta.insert_str(&replacement);
    } else {
        ta.insert_str(&format!("{open}{close}"));
        for _ in 0..close.chars().count() {
            ta.move_cursor(CursorMove::Back);
        }
    }
}

fn toggle_wrap(s: &str, open: &str, close: &str) -> String {
    if s.len() >= open.len() + close.len() {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            return inner.to_string();
        }
    }
    format!("{open}{s}{close}")
}

/// Select `[start_char, end_char)` in joined content and replace with `text`.
/// Leaves the cursor at the end of the inserted text.
pub fn replace_char_range(ta: &mut TextArea<'_>, start_char: usize, end_char: usize, text: &str) {
    let lines: Vec<String> = ta.lines().to_vec();
    let (sr, sc) = char_to_rowcol(&lines, start_char);
    let (er, ec) = char_to_rowcol(&lines, end_char);

    ta.cancel_selection();
    ta.move_cursor(CursorMove::Jump(sr as u16, sc as u16));
    if start_char != end_char {
        ta.start_selection();
        ta.move_cursor(CursorMove::Jump(er as u16, ec as u16));
        ta.cut();
    }
    ta.insert_str(text);
}

/// Active color autocomplete query at the current cursor, if any.
pub fn active_color_query(ta: &TextArea<'_>) -> Option<ColorQuery> {
    let content = ta.lines().join("\n");
    let cur = cursor_char_offset(ta);
    markup::color_query_at(&content, cur)
}

/// Complete the open `{prefix` tag to `{name}…{/}` with the cursor between.
///
/// If the user already typed a closing `}` we only finish the name; otherwise
/// we insert `{name}{/}` and place the caret in the middle.
pub fn apply_color_completion(ta: &mut TextArea<'_>, query: &ColorQuery, name: &str) {
    let content = ta.lines().join("\n");
    let chars: Vec<char> = content.chars().collect();
    // Replace from `{` through cursor with `{name}`
    let open = format!("{{{name}}}");
    let open_len = open.chars().count();
    // If there is already a `{/` after the cursor soon, just fill the open tag.
    let rest: String = chars.get(query.cursor_char..).unwrap_or(&[]).iter().collect();
    let replacement = if rest.starts_with("{/") || rest.starts_with('}') {
        open
    } else {
        format!("{{{name}}}{{/}}")
    };

    replace_char_range(ta, query.brace_char, query.cursor_char, &replacement);

    // Place cursor after `{name}` (between color tags when we inserted `{/}`).
    let after_open = query.brace_char + open_len;
    let lines: Vec<String> = ta.lines().to_vec();
    let (r, c) = char_to_rowcol(&lines, after_open);
    ta.cancel_selection();
    ta.move_cursor(CursorMove::Jump(r as u16, c as u16));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_textarea::TextArea;

    #[test]
    fn wrap_inserts_and_places_cursor() {
        let mut ta = TextArea::default();
        wrap_markup(&mut ta, "**", "**");
        assert_eq!(ta.lines().join("\n"), "****");
        // cursor between
        assert_eq!(ta.cursor(), (0, 2));
    }

    #[test]
    fn wrap_selection_toggles() {
        let mut ta = TextArea::from(["hello"]);
        ta.move_cursor(CursorMove::Jump(0, 0));
        ta.start_selection();
        ta.move_cursor(CursorMove::Jump(0, 5));
        wrap_markup(&mut ta, "**", "**");
        assert_eq!(ta.lines().join("\n"), "**hello**");
        // select all and toggle off
        ta.select_all();
        wrap_markup(&mut ta, "**", "**");
        assert_eq!(ta.lines().join("\n"), "hello");
    }

    #[test]
    fn color_complete_inserts_pair() {
        let mut ta = TextArea::from(["say {re"]);
        ta.move_cursor(CursorMove::Jump(0, 7));
        let q = active_color_query(&ta).expect("query");
        assert_eq!(q.prefix, "re");
        apply_color_completion(&mut ta, &q, "red");
        assert_eq!(ta.lines().join("\n"), "say {red}{/}");
        // cursor between tags
        assert_eq!(ta.cursor(), (0, 9));
    }

    #[test]
    fn char_offsets_roundtrip() {
        let lines = vec!["ab".into(), "cde".into()];
        assert_eq!(char_to_rowcol(&lines, 0), (0, 0));
        assert_eq!(char_to_rowcol(&lines, 2), (0, 2));
        assert_eq!(char_to_rowcol(&lines, 3), (1, 0));
        assert_eq!(char_to_rowcol(&lines, 5), (1, 2));
    }
}
