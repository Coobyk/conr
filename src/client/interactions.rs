//! Quote posts, command palette, and @mention autocomplete for the composer.

use crate::shared::{Post, QuotedPost};

use crate::client::app::{App, ReturnTo, Screen};
use crate::client::phase_c::SearchHit;

pub fn quoted(post: &Post) -> QuotedPost {
    QuotedPost {
        id: post.id.clone(),
        author_handle: post.author_handle.clone(),
        content: if post.deleted_at.is_some() {
            String::new()
        } else {
            post.content.clone()
        },
        deleted_at: post.deleted_at,
        images: if post.deleted_at.is_some() {
            Vec::new()
        } else {
            post.images.clone()
        },
    }
}

pub fn update_post(list: &mut [Post], post: &Post) {
    for p in list {
        if p.id == post.id {
            *p = post.clone();
        } else if p.quote_id.as_deref() == Some(&post.id) {
            p.quote = Some(quoted(post));
        }
    }
}

/// Truncate to `width` chars, appending `…` when cut.
pub fn truncate_chars(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// One-line quote preview for list rows. `None` when the post itself is gone
/// or never had a quote.
pub fn quote_line(post: &Post, width: usize) -> Option<String> {
    if post.deleted_at.is_some() || post.quote_id.is_none() {
        return None;
    }
    let line = match &post.quote {
        None => "» quoted post unavailable".to_string(),
        Some(q) if q.deleted_at.is_some() => "» quoted post unavailable".to_string(),
        Some(q) => {
            let body = if !q.content.trim().is_empty() {
                crate::client::ui::markup::strip_markup(&q.content)
            } else if !q.images.is_empty() {
                let n = q.images.len();
                if n == 1 {
                    "1 image".to_string()
                } else {
                    format!("{n} images")
                }
            } else {
                String::new()
            };
            if body.is_empty() {
                format!("» @{}", q.author_handle)
            } else {
                format!("» @{} {}", q.author_handle, body)
            }
        }
    };
    Some(truncate_chars(&line, width))
}

/// Preview line for the compose pending-quote strip.
pub fn quoted_preview(q: &QuotedPost, width: usize) -> String {
    let line = if q.deleted_at.is_some() {
        format!("» @{} unavailable", q.author_handle)
    } else {
        let body = if !q.content.trim().is_empty() {
            crate::client::ui::markup::strip_markup(&q.content)
        } else if !q.images.is_empty() {
            let n = q.images.len();
            if n == 1 {
                "1 image".to_string()
            } else {
                format!("{n} images")
            }
        } else {
            String::new()
        };
        if body.is_empty() {
            format!("» @{}", q.author_handle)
        } else {
            format!("» @{} {}", q.author_handle, body)
        }
    };
    truncate_chars(&line, width)
}

/// Live @mention autocomplete state for the composer.
#[derive(Default, Debug)]
pub struct MentionAc {
    pub query: Option<String>,
    pub start: usize,
    pub items: Vec<String>,
    pub selected: usize,
    pub request: u64,
}

/// Max rows shown in the mention dropdown.
pub const MENTION_MAX_ITEMS: usize = 6;

/// Extract the `@handle` token immediately before the cursor, if any.
///
/// Returns `(query, start_char_offset)` where `start` points at the `@`.
/// The token must be a bare mention: preceded by start-of-line/whitespace and
/// containing only handle characters up to the cursor. A bare `@` (empty
/// query) is allowed so the dropdown can open as soon as `@` is typed.
pub fn mention_query_at(content: &str, cursor: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = content.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 {
        let c = chars[start - 1];
        if c == '@' {
            break;
        }
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return None;
        }
        start -= 1;
    }
    if start == 0 || chars.get(start - 1) != Some(&'@') {
        return None;
    }
    if start >= 2 && !chars[start - 2].is_whitespace() {
        return None;
    }
    if start >= 2 && chars[start - 2] == '@' {
        return None;
    }
    let query: String = chars[start..cursor].iter().collect();
    if query.chars().count() > 32 {
        return None;
    }
    Some((query, start - 1))
}

/// Apply a mention completion: replace `@query` (from `start` to cursor) with
/// `@handle ` and return the cursor char offset just after the trailing space.
pub fn apply_mention(
    ta: &mut tui_textarea::TextArea<'_>,
    start: usize,
    end: usize,
    handle: &str,
) -> usize {
    let replacement = format!("@{handle} ");
    crate::client::ui::compose_edit::replace_char_range(ta, start, end, &replacement);
    start + replacement.chars().count()
}

impl App {
    /// Refresh mention autocomplete after a text change in the composer.
    pub fn composer_mention_tick(&mut self) {
        let (content, cursor, editing_quote) = match &self.screen {
            Screen::Compose {
                textarea,
                quote,
                edit_id,
                ..
            } => (
                textarea.lines().join("\n"),
                crate::client::ui::compose_edit::cursor_char_offset(textarea),
                quote.is_some() && edit_id.is_none(),
            ),
            _ => return,
        };
        if !editing_quote {
            return;
        }
        let found = mention_query_at(&content, cursor);
        let ac = match &mut self.screen {
            Screen::Compose { mention_ac, .. } => mention_ac,
            _ => return,
        };
        match found {
            Some((query, start)) => {
                if ac.query.as_deref() != Some(query.as_str()) || ac.start != start {
                    ac.request += 1;
                    let request = ac.request;
                    ac.query = Some(query.clone());
                    ac.start = start;
                    ac.selected = 0;
                    self.spawn_mention_search(query, request);
                }
            }
            None => *ac = MentionAc::default(),
        }
    }
}

impl App {
    pub fn open_quote_compose(&mut self, post: &Post) {
        let quote = quoted(post);
        let deleted = quote.deleted_at.is_some();
        let handle = quote.author_handle.clone();
        let mut ta = tui_textarea::TextArea::default();
        let prompt = if deleted {
            format!("quote @{handle}'s post is unavailable")
        } else {
            ta.set_placeholder_text("add a comment");
            format!("quote @{handle}")
        };
        self.screen = Screen::Compose {
            textarea: ta,
            error: None,
            parent_id: None,
            quote: Some(quote),
            mention_ac: crate::client::interactions::MentionAc::default(),
            edit_id: None,
            return_to: ReturnTo::Feed,
            color_ac_idx: 0,
            images: Vec::new(),
            attach_path: None,
        };
        self.status = prompt;
    }
}

// ── Command palette ─────────────────────────────────────────────────────────

/// Identifier for a palette command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Feed,
    Following,
    NewPost,
    Search,
    Notifications,
    Dms,
    Profile,
    Refresh,
    Help,
    Quote,
    Logout,
}

/// All palette commands with their display labels (fixed order).
pub fn palette_commands() -> Vec<(CommandId, &'static str)> {
    vec![
        (CommandId::Feed, "Go to feed"),
        (CommandId::Following, "Following tab"),
        (CommandId::NewPost, "New post"),
        (CommandId::Search, "Search"),
        (CommandId::Notifications, "Notifications"),
        (CommandId::Dms, "Direct messages"),
        (CommandId::Profile, "Your profile"),
        (CommandId::Refresh, "Refresh"),
        (CommandId::Help, "Help"),
        (CommandId::Quote, "Quote selected post"),
        (CommandId::Logout, "Log out"),
    ]
}

/// Fuzzy-filter palette commands by `query` (empty = all, original order).
pub fn filter_palette(query: &str) -> Vec<CommandId> {
    let commands = palette_commands();
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return commands.iter().map(|(id, _)| *id).collect();
    }
    use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
    use nucleo_matcher::{Config, Matcher, Utf32Str};
    let pattern = Pattern::parse(trimmed, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(usize, u32)> = Vec::new();
    for (i, (_, label)) in commands.iter().enumerate() {
        buf.clear();
        if let Some(score) = pattern.score(Utf32Str::new(label, &mut buf), &mut matcher) {
            scored.push((i, score));
        }
    }
    scored.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
    scored.into_iter().map(|(i, _)| commands[i].0).collect()
}

impl App {
    /// True when Ctrl+P / Ctrl+K may open the palette on the current screen.
    pub fn palette_openable(&self) -> bool {
        matches!(
            self.screen,
            Screen::Feed
                | Screen::Thread
                | Screen::Search { .. }
                | Screen::Profile { .. }
                | Screen::Notifications { .. }
                | Screen::DmList { .. }
                | Screen::Help { .. }
        )
    }

    /// Overlay the palette on the current screen.
    pub fn open_palette(&mut self) {
        let prev = std::mem::replace(&mut self.screen, Screen::Boot);
        self.screen = Screen::Palette {
            query: String::new(),
            selected: 0,
            prev: Box::new(prev),
        };
    }

    /// Restore the screen that was under the palette.
    pub fn close_palette(&mut self) {
        if let Screen::Palette { prev, .. } = std::mem::replace(&mut self.screen, Screen::Boot) {
            self.screen = *prev;
        }
    }

    /// Selected post for palette Quote — walks through a palette overlay.
    pub fn selected_post(&self) -> Option<&Post> {
        selected_post_in(&self.screen, self)
    }

    /// Handle one key while the palette is open.
    pub async fn handle_palette_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyModifiers};

        let list = match &self.screen {
            Screen::Palette { query, .. } => filter_palette(query),
            _ => return Ok(()),
        };
        let n = list.len();

        match key.code {
            KeyCode::Esc => {
                self.close_palette();
                return Ok(());
            }
            KeyCode::Enter => {
                if n == 0 {
                    return Ok(());
                }
                let id = match &self.screen {
                    Screen::Palette { selected, .. } => list[(*selected).min(n - 1)],
                    _ => return Ok(()),
                };
                self.close_palette();
                self.run_palette_command(id).await;
                return Ok(());
            }
            KeyCode::Down | KeyCode::Char('j')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if n > 0 {
                    if let Screen::Palette { selected, .. } = &mut self.screen {
                        *selected = (*selected + 1) % n;
                    }
                }
                return Ok(());
            }
            KeyCode::Up | KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if n > 0 {
                    if let Screen::Palette { selected, .. } = &mut self.screen {
                        *selected = selected.checked_sub(1).unwrap_or(n - 1);
                    }
                }
                return Ok(());
            }
            KeyCode::Backspace => {
                if let Screen::Palette {
                    query, selected, ..
                } = &mut self.screen
                {
                    query.pop();
                    *selected = 0;
                }
                return Ok(());
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Screen::Palette {
                    query, selected, ..
                } = &mut self.screen
                {
                    query.push(c);
                    *selected = 0;
                }
                return Ok(());
            }
            _ => {}
        }
        Ok(())
    }

    /// Dispatch a palette command. Call after `close_palette()`.
    pub async fn run_palette_command(&mut self, id: CommandId) {
        match id {
            CommandId::Feed => self.go_home(),
            CommandId::Following => {
                self.go_home();
                if self.feed_tab != crate::client::phase_c::FeedTab::Following {
                    self.switch_feed_tab();
                }
            }
            CommandId::NewPost => {
                let ret = if matches!(self.screen, Screen::Thread) {
                    ReturnTo::Thread
                } else {
                    ReturnTo::Feed
                };
                self.open_compose(None, ret);
            }
            CommandId::Search => self.open_search().await,
            CommandId::Notifications => {
                if let Err(e) = self.open_notifications().await {
                    self.status = format!("notifications: {e:#}");
                }
            }
            CommandId::Dms => {
                if let Err(e) = self.open_dm_list().await {
                    self.status = format!("dms: {e:#}");
                }
            }
            CommandId::Profile => {
                let handle = self.session.as_ref().map(|s| s.handle.clone());
                match handle {
                    Some(h) => {
                        if let Err(e) = self.open_profile(&h).await {
                            self.status = format!("profile: {e:#}");
                        }
                    }
                    None => self.status = "not logged in".into(),
                }
            }
            CommandId::Refresh => self.refresh_current().await,
            CommandId::Help => {
                let ret = if matches!(self.screen, Screen::Thread) {
                    ReturnTo::Thread
                } else {
                    ReturnTo::Feed
                };
                self.screen = Screen::Help { return_to: ret };
            }
            CommandId::Quote => match self.selected_post().cloned() {
                Some(post) if post.deleted_at.is_some() => {
                    self.status = "cannot quote a deleted post".into();
                }
                Some(post) => self.open_quote_compose(&post),
                None => self.status = "no post selected".into(),
            },
            CommandId::Logout => self.logout(),
        }
    }

    /// Refresh whatever list/view is currently on screen.
    pub async fn refresh_current(&mut self) {
        match &self.screen {
            Screen::Feed => {
                self.status = "refreshing…".into();
                self.spawn_refresh_feed();
            }
            Screen::Thread => {
                let rid = self.thread.as_ref().map(|t| t.root_id.clone());
                if let Some(rid) = rid {
                    if let Err(e) = self.open_thread(&rid).await {
                        self.status = format!("refresh: {e:#}");
                    }
                } else {
                    self.go_home();
                }
            }
            Screen::Profile { state } => {
                let h = state.profile.handle.clone();
                self.status = "refreshing profile…".into();
                self.spawn_load_profile(h);
            }
            Screen::Notifications { .. } => {
                self.status = "refreshing notifications…".into();
                self.spawn_load_notifications();
            }
            Screen::DmList { .. } => {
                self.status = "refreshing messages…".into();
                self.spawn_load_dm_list();
            }
            Screen::Search { state } => {
                let q = state.query.clone();
                if q.trim().is_empty() {
                    self.status = "type a query first".into();
                    return;
                }
                match self.api.search(&q).await {
                    Ok(resp) => {
                        let hits = crate::client::phase_c::flatten_search(resp);
                        if let Screen::Search { state } = &mut self.screen {
                            state.hits = hits;
                            state.selected = 0;
                            state.error = None;
                            state.editing = false;
                        }
                        self.status = "search refreshed".into();
                    }
                    Err(e) => self.status = format!("search: {e:#}"),
                }
            }
            Screen::Help { return_to } => {
                let ret = *return_to;
                match ret {
                    ReturnTo::Feed => {
                        self.status = "refreshing…".into();
                        self.spawn_refresh_feed();
                    }
                    ReturnTo::Thread => {
                        let rid = self.thread.as_ref().map(|t| t.root_id.clone());
                        if let Some(rid) = rid {
                            if let Err(e) = self.open_thread(&rid).await {
                                self.status = format!("refresh: {e:#}");
                            }
                        } else {
                            self.go_home();
                        }
                    }
                }
            }
            _ => self.status = "nothing to refresh".into(),
        }
    }
}

fn selected_post_in<'a>(screen: &'a Screen, app: &'a App) -> Option<&'a Post> {
    match screen {
        Screen::Feed => app.posts.get(app.selected),
        Screen::Thread => app.thread.as_ref().and_then(|t| t.posts.get(t.selected)),
        Screen::Profile { state } => state.posts.get(state.selected),
        Screen::Search { state } => match state.hits.get(state.selected) {
            Some(SearchHit::Post(p)) => Some(p),
            _ => None,
        },
        Screen::Palette { prev, .. } => selected_post_in(prev, app),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::MediaInfo;

    fn sample_post(quote_id: Option<&str>, quote: Option<QuotedPost>) -> Post {
        Post {
            id: "p1".into(),
            author_id: "u1".into(),
            author_handle: "alice".into(),
            parent_id: None,
            quote_id: quote_id.map(|s| s.to_string()),
            quote,
            content: "hello world".into(),
            created_at: chrono::Utc::now(),
            deleted_at: None,
            edited_at: None,
            like_count: 0,
            liked_by_me: false,
            reply_count: 0,
            images: Vec::new(),
        }
    }

    fn sample_quote(content: &str, images: Vec<MediaInfo>) -> QuotedPost {
        QuotedPost {
            id: "q1".into(),
            author_handle: "bob".into(),
            content: content.into(),
            deleted_at: None,
            images,
        }
    }

    #[test]
    fn mention_query_at_bare_at_opens() {
        assert_eq!(mention_query_at("hi @", 4), Some((String::new(), 3)));
        assert_eq!(mention_query_at("@", 1), Some((String::new(), 0)));
    }

    #[test]
    fn mention_query_at_partial_handle() {
        assert_eq!(mention_query_at("hi @al", 6), Some(("al".into(), 3)));
        assert_eq!(mention_query_at("@al", 3), Some(("al".into(), 0)));
        assert_eq!(mention_query_at("say @a_b", 8), Some(("a_b".into(), 4)));
    }

    #[test]
    fn mention_query_at_rejects_bad_context() {
        assert_eq!(mention_query_at("mail@x", 6), None);
        assert_eq!(mention_query_at("a@b", 3), None);
        assert_eq!(mention_query_at("x@@y", 4), None);
        assert_eq!(mention_query_at("hi @Al", 6), None);
        assert_eq!(mention_query_at("no at here", 10), None);
    }

    #[test]
    fn apply_mention_cursor_after_trailing_space() {
        let mut ta = tui_textarea::TextArea::from(["hi @al"]);
        ta.move_cursor(tui_textarea::CursorMove::Jump(0, 6));
        let cursor = crate::client::ui::compose_edit::cursor_char_offset(&ta);
        let new_cursor = apply_mention(&mut ta, 3, cursor, "alice");
        let content = ta.lines().join("\n");
        assert_eq!(content, "hi @alice ");
        assert_eq!(new_cursor, 3 + "@alice ".chars().count());
        assert_eq!(mention_query_at(&content, new_cursor), None);
    }

    #[test]
    fn quote_line_none_without_quote() {
        let p = sample_post(None, None);
        assert_eq!(quote_line(&p, 40), None);
        let mut deleted = sample_post(Some("q1"), None);
        deleted.deleted_at = Some(chrono::Utc::now());
        assert_eq!(quote_line(&deleted, 40), None);
    }

    #[test]
    fn quote_line_unavailable_when_quote_missing() {
        let p = sample_post(Some("q1"), None);
        let line = quote_line(&p, 40).expect("line");
        assert!(line.contains("unavailable"), "{line}");
    }

    #[test]
    fn quote_line_truncates_with_ellipsis() {
        let long = "x".repeat(80);
        let p = sample_post(Some("q1"), Some(sample_quote(&long, Vec::new())));
        let line = quote_line(&p, 20).expect("line");
        assert!(line.chars().count() <= 20, "{line}");
        assert!(line.ends_with('…'), "{line}");
    }

    #[test]
    fn quote_line_image_count_fallback() {
        let img = MediaInfo {
            id: "m1".into(),
            content_type: "image/png".into(),
            size_bytes: 1,
            width: None,
            height: None,
        };
        let p = sample_post(Some("q1"), Some(sample_quote("", vec![img.clone(), img])));
        let line = quote_line(&p, 60).expect("line");
        assert!(line.contains("2 images"), "{line}");
        assert!(line.contains("@bob"), "{line}");
    }

    #[test]
    fn truncate_chars_respects_width() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 5), "hello");
        let t = truncate_chars("hello", 4);
        assert_eq!(t.chars().count(), 4);
        assert!(t.ends_with('…'));
        assert_eq!(truncate_chars("hello", 0), "");
        assert_eq!(truncate_chars("hello", 1), "…");
    }

    #[test]
    fn quoted_preview_shows_handle() {
        let q = sample_quote("body text", Vec::new());
        let p = quoted_preview(&q, 40);
        assert!(p.contains("@bob"), "{p}");
        assert!(p.contains("body text"), "{p}");
    }

    #[test]
    fn palette_empty_query_returns_all_in_order() {
        let all = filter_palette("");
        assert_eq!(all.len(), palette_commands().len());
        assert_eq!(all[0], CommandId::Feed);
        let all_ws = filter_palette("   ");
        assert_eq!(all_ws.len(), palette_commands().len());
    }

    #[test]
    fn palette_fuzzy_filters() {
        let f = filter_palette("new");
        assert!(f.contains(&CommandId::NewPost), "{f:?}");
        let f2 = filter_palette("np");
        assert!(f2.contains(&CommandId::NewPost), "{f2:?}");
        let none = filter_palette("zzzzqqq");
        assert!(none.is_empty());
    }

    #[test]
    fn palette_quote_and_logout_present() {
        let all = filter_palette("");
        assert!(all.contains(&CommandId::Quote));
        assert!(all.contains(&CommandId::Logout));
        assert!(all.contains(&CommandId::Help));
    }
}
