//! Phase C screens: search, profile, notifications, DMs, feed tab helpers.

use anyhow::Result;
use conr_shared::{
    DmConversation, DmMessage, Notification, BLOCK_REASON_MAX_LEN, POST_MAX_LEN, SearchResponse,
    UserProfile,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::TextArea;

use crate::app::{App, ReturnTo, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedTab {
    All,
    Following,
}

impl FeedTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "For you",
            Self::Following => "Following",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::All => Self::Following,
            Self::Following => Self::All,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SearchHit {
    Post(conr_shared::Post),
    User(UserProfile),
    Tag(String, i64),
}

#[derive(Debug)]
pub struct SearchState {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub selected: usize,
    pub error: Option<String>,
    /// true = editing query, false = browsing results
    pub editing: bool,
}

#[derive(Debug)]
pub struct ProfileState {
    pub profile: UserProfile,
    pub posts: Vec<conr_shared::Post>,
    pub selected: usize,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug)]
pub struct NotifState {
    pub items: Vec<Notification>,
    pub selected: usize,
    #[allow(dead_code)]
    pub unread: i64,
}

#[derive(Debug)]
pub struct DmListState {
    pub items: Vec<DmConversation>,
    pub selected: usize,
}

/// Full chat thread with a peer.
#[derive(Debug)]
pub struct DmThreadState {
    pub conversation_id: String,
    pub peer_handle: String,
    pub messages: Vec<DmMessage>,
    pub draft: TextArea<'static>,
    pub error: Option<String>,
    pub scroll: usize,
}

/// Step 1 of new message: pick a handle.
#[derive(Debug)]
pub struct DmNewState {
    pub handle: String,
    pub error: Option<String>,
}

impl App {
    pub async fn handle_phase_c_key(&mut self, key: KeyEvent) -> Result<bool> {
        match &self.screen {
            Screen::Search { .. } => {
                self.handle_search_key(key).await?;
                Ok(true)
            }
            Screen::Profile { .. } => {
                self.handle_profile_key(key).await?;
                Ok(true)
            }
            Screen::Notifications { .. } => {
                self.handle_notif_key(key).await?;
                Ok(true)
            }
            Screen::DmList { .. } => {
                self.handle_dm_list_key(key).await?;
                Ok(true)
            }
            Screen::DmThread { .. } => {
                self.handle_dm_thread_key(key).await?;
                Ok(true)
            }
            Screen::DmNew { .. } => {
                self.handle_dm_new_key(key).await?;
                Ok(true)
            }
            Screen::EditBio { .. } => {
                self.handle_edit_bio_key(key).await?;
                Ok(true)
            }
            Screen::BlockUser { .. } => {
                self.handle_block_user_key(key).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub async fn open_search(&mut self) {
        self.screen = Screen::Search {
            state: SearchState {
                query: String::new(),
                hits: Vec::new(),
                selected: 0,
                error: None,
                editing: true,
            },
        };
        self.status = "search".into();
    }

    async fn handle_search_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc {
            self.go_home();
            self.status = "timeline".into();
            return Ok(());
        }

        let editing = matches!(
            &self.screen,
            Screen::Search {
                state: SearchState { editing: true, .. }
            }
        );

        if editing {
            match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Screen::Search { state } = &mut self.screen {
                        state.query.push(c);
                        state.error = None;
                    }
                }
                KeyCode::Backspace => {
                    if let Screen::Search { state } = &mut self.screen {
                        state.query.pop();
                    }
                }
                KeyCode::Enter => {
                    let q = if let Screen::Search { state } = &self.screen {
                        state.query.clone()
                    } else {
                        return Ok(());
                    };
                    if q.trim().is_empty() {
                        if let Screen::Search { state } = &mut self.screen {
                            state.error = Some("type something first".into());
                        }
                        return Ok(());
                    }
                    match self.api.search(&q).await {
                        Ok(resp) => {
                            let hits = flatten_search(resp);
                            if let Screen::Search { state } = &mut self.screen {
                                state.hits = hits;
                                state.selected = 0;
                                state.error = None;
                                state.editing = false;
                            }
                            self.status = "↑↓ select · Enter open · / retype".into();
                        }
                        Err(e) => {
                            if let Screen::Search { state } = &mut self.screen {
                                state.error = Some(format!("{e:#}"));
                            }
                        }
                    }
                }
                KeyCode::Down if {
                    matches!(
                        &self.screen,
                        Screen::Search {
                            state: SearchState { hits, .. }
                        } if !hits.is_empty()
                    )
                } =>
                {
                    if let Screen::Search { state } = &mut self.screen {
                        state.editing = false;
                        state.selected = 0;
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        // browsing results
        match key.code {
            KeyCode::Char('/') => {
                if let Screen::Search { state } = &mut self.screen {
                    state.editing = true;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Screen::Search { state } = &mut self.screen {
                    if !state.hits.is_empty() {
                        state.selected = (state.selected + 1).min(state.hits.len() - 1);
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Screen::Search { state } = &mut self.screen {
                    if state.selected == 0 {
                        state.editing = true;
                    } else {
                        state.selected = state.selected.saturating_sub(1);
                    }
                }
            }
            KeyCode::Enter => {
                let hit = if let Screen::Search { state } = &self.screen {
                    state.hits.get(state.selected).cloned()
                } else {
                    None
                };
                if let Some(hit) = hit {
                    match hit {
                        SearchHit::Post(p) => {
                            if let Err(e) = self.open_thread(&p.id).await {
                                self.status = format!("thread: {e:#}");
                            }
                        }
                        SearchHit::User(u) => {
                            if let Err(e) = self.open_profile(&u.handle).await {
                                self.status = format!("profile: {e:#}");
                            }
                        }
                        SearchHit::Tag(name, _) => {
                            let q = format!("#{name}");
                            if let Ok(resp) = self.api.search(&q).await {
                                if let Screen::Search { state } = &mut self.screen {
                                    state.query = q;
                                    state.hits = flatten_search(resp);
                                    state.selected = 0;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn open_profile(&mut self, handle: &str) -> Result<()> {
        let handle = handle
            .trim()
            .trim_start_matches('@')
            .to_ascii_lowercase();
        if handle.is_empty() {
            anyhow::bail!("empty handle");
        }
        self.pending_profile = Some(handle.clone());
        let is_me = self
            .session
            .as_ref()
            .map(|s| s.handle.eq_ignore_ascii_case(&handle))
            .unwrap_or(false);

        // Instant: memory → disk posts → empty shell.
        if let Some(cached) = self.view_cache.profile(&handle) {
            self.screen = Screen::Profile {
                state: ProfileState {
                    profile: cached.profile.clone(),
                    posts: cached.posts.clone(),
                    selected: 0,
                    next_cursor: cached.next_cursor.clone(),
                    has_more: cached.has_more,
                },
            };
            self.status = format!("@{handle}");
        } else {
            let posts = self.cache.by_author(&handle, 40).unwrap_or_default();
            let profile = App::stub_profile(&handle, is_me);
            self.screen = Screen::Profile {
                state: ProfileState {
                    profile,
                    posts,
                    selected: 0,
                    next_cursor: None,
                    has_more: true,
                },
            };
            self.status = format!("@{handle} · loading…");
        }
        self.spawn_load_profile(handle);
        Ok(())
    }

    async fn handle_profile_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.go_home();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Screen::Profile { state } = &mut self.screen {
                    if !state.posts.is_empty() {
                        state.selected = (state.selected + 1).min(state.posts.len() - 1);
                    }
                }
                self.maybe_load_more_profile();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Screen::Profile { state } = &mut self.screen {
                    state.selected = state.selected.saturating_sub(1);
                }
            }
            KeyCode::Char('g') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                if let Screen::Profile { state } = &mut self.screen {
                    state.selected = 0;
                }
            }
            KeyCode::Char('G') | KeyCode::Char('g')
                if key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                if let Screen::Profile { state } = &mut self.screen {
                    if !state.posts.is_empty() {
                        state.selected = state.posts.len() - 1;
                    }
                }
                self.maybe_load_more_profile();
            }
            KeyCode::Enter => {
                let id = if let Screen::Profile { state } = &self.screen {
                    state.posts.get(state.selected).map(|p| p.id.clone())
                } else {
                    None
                };
                if let Some(id) = id {
                    let _ = self.open_thread(&id).await;
                }
            }
            KeyCode::Char('f') => {
                let (handle, following, is_me) = if let Screen::Profile { state } = &self.screen {
                    (
                        state.profile.handle.clone(),
                        state.profile.followed_by_me,
                        state.profile.is_me,
                    )
                } else {
                    return Ok(());
                };
                if is_me {
                    self.status = "that's you".into();
                    return Ok(());
                }
                let res = if following {
                    self.api.unfollow(&handle).await
                } else {
                    self.api.follow(&handle).await
                };
                match res {
                    Ok(p) => {
                        if following {
                            self.following_handles.remove(&handle);
                        } else {
                            self.following_handles.insert(handle.clone());
                        }
                        if let Screen::Profile { state } = &mut self.screen {
                            state.profile = p;
                        }
                        self.status = if following {
                            format!("unfollowed @{handle}")
                        } else {
                            format!("followed @{handle}")
                        };
                    }
                    Err(e) => self.status = format!("follow: {e:#}"),
                }
            }
            KeyCode::Char('b') => {
                let (handle, blocked, is_me) = if let Screen::Profile { state } = &self.screen {
                    (
                        state.profile.handle.clone(),
                        state.profile.blocked_by_me,
                        state.profile.is_me,
                    )
                } else {
                    return Ok(());
                };
                if is_me {
                    self.status = "can't block yourself".into();
                    return Ok(());
                }
                if blocked {
                    if let Err(e) = self.api.unblock(&handle).await {
                        self.status = format!("unblock: {e:#}");
                    } else {
                        self.blocked_handles.remove(&handle);
                        let _ = self.open_profile(&handle).await;
                        self.status = format!("unblocked @{handle}");
                    }
                } else {
                    // Require a written reason before the block is sent.
                    self.screen = Screen::BlockUser {
                        handle,
                        reason: String::new(),
                        error: None,
                    };
                    self.status = "write a reason for this block".into();
                }
            }
            KeyCode::Char('m') => {
                let (handle, is_me) = if let Screen::Profile { state } = &self.screen {
                    (state.profile.handle.clone(), state.profile.is_me)
                } else {
                    return Ok(());
                };
                if is_me {
                    self.status = "can't DM yourself".into();
                    return Ok(());
                }
                if let Err(e) = self.open_dm_with_peer(&handle).await {
                    self.status = format!("dm: {e:#}");
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                let is_me = if let Screen::Profile { state } = &self.screen {
                    state.profile.is_me
                } else {
                    false
                };
                if is_me {
                    let bio = if let Screen::Profile { state } = &self.screen {
                        state.profile.bio.clone().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    self.screen = Screen::EditBio {
                        input: bio,
                        error: None,
                    };
                } else {
                    self.status = "you can only edit your own bio".into();
                }
            }
            KeyCode::Char(' ') => {
                let id = if let Screen::Profile { state } = &self.screen {
                    state.posts.get(state.selected).map(|p| p.id.clone())
                } else {
                    None
                };
                if let Some(id) = id {
                    self.toggle_like_on(&id).await;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn open_notifications(&mut self) -> Result<()> {
        // Instant from memory; mark-read + refresh run in the background.
        if let Some(items) = self.view_cache.notifications() {
            self.screen = Screen::Notifications {
                state: NotifState {
                    items: items.to_vec(),
                    selected: 0,
                    unread: 0,
                },
            };
            self.unread_notifs = 0;
            self.status = "notifications".into();
        } else {
            self.screen = Screen::Notifications {
                state: NotifState {
                    items: Vec::new(),
                    selected: 0,
                    unread: 0,
                },
            };
            self.status = "loading notifications…".into();
        }
        self.spawn_load_notifications();
        Ok(())
    }

    async fn handle_notif_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.go_home(),
            KeyCode::Char('j') | KeyCode::Down => {
                if let Screen::Notifications { state } = &mut self.screen {
                    if !state.items.is_empty() {
                        state.selected = (state.selected + 1).min(state.items.len() - 1);
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Screen::Notifications { state } = &mut self.screen {
                    state.selected = state.selected.saturating_sub(1);
                }
            }
            KeyCode::Enter => {
                let post_id = if let Screen::Notifications { state } = &self.screen {
                    state
                        .items
                        .get(state.selected)
                        .and_then(|n| n.post_id.clone())
                } else {
                    None
                };
                let actor = if let Screen::Notifications { state } = &self.screen {
                    state
                        .items
                        .get(state.selected)
                        .map(|n| n.actor_handle.clone())
                } else {
                    None
                };
                if let Some(pid) = post_id {
                    let _ = self.open_thread(&pid).await;
                } else if let Some(h) = actor {
                    let _ = self.open_profile(&h).await;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── Direct messages ─────────────────────────────────────────────────────

    pub async fn open_dm_list(&mut self) -> Result<()> {
        if let Some(items) = self.view_cache.dm_list() {
            self.screen = Screen::DmList {
                state: DmListState {
                    items: items.to_vec(),
                    selected: 0,
                },
            };
            self.status = "messages · n new · Enter open".into();
        } else {
            self.screen = Screen::DmList {
                state: DmListState {
                    items: Vec::new(),
                    selected: 0,
                },
            };
            self.status = "loading messages…".into();
        }
        self.spawn_load_dm_list();
        Ok(())
    }

    /// Open chat with a known peer (from profile, etc.). Reuses existing convo if any.
    pub async fn open_dm_with_peer(&mut self, handle: &str) -> Result<()> {
        let handle = handle.trim().trim_start_matches('@').to_ascii_lowercase();
        if handle.is_empty() {
            anyhow::bail!("empty handle");
        }

        // Memory: full thread by peer.
        if let Some(th) = self.view_cache.dm_thread_by_peer(&handle) {
            let conv_id = th.conversation_id.clone();
            let peer = th.peer_handle.clone();
            let messages = th.messages.clone();
            let n = messages.len();
            self.pending_dm = Some(conv_id.clone());
            self.note_dm_opened(&conv_id);
            let mut draft = TextArea::default();
            draft.set_placeholder_text("Write a message…");
            self.screen = Screen::DmThread {
                state: DmThreadState {
                    conversation_id: conv_id.clone(),
                    peer_handle: peer,
                    messages,
                    draft,
                    error: None,
                    scroll: n.saturating_sub(1),
                },
            };
            self.status = format!("@{} · chat", handle);
            self.spawn_load_dm_thread(conv_id.clone(), conv_id);
            return Ok(());
        }

        // Memory: DM list entry → open by id.
        let existing_id = self.view_cache.dm_list().and_then(|list| {
            list.iter()
                .find(|c| c.peer_handle == handle)
                .map(|c| c.id.clone())
        });
        if let Some(id) = existing_id {
            return self.open_dm_thread(&id).await;
        }

        // New empty chat UI immediately; resolve existing convo in the background.
        let request_key = format!("peer:{handle}");
        self.pending_dm = Some(request_key.clone());
        let mut draft = TextArea::default();
        draft.set_placeholder_text("Write a message…");
        self.screen = Screen::DmThread {
            state: DmThreadState {
                conversation_id: String::new(),
                peer_handle: handle.clone(),
                messages: Vec::new(),
                draft,
                error: None,
                scroll: 0,
            },
        };
        self.status = "new chat · Enter send · Esc back".into();

        let api = self.api.clone();
        let tx = self.event_tx.clone();
        let peer = handle.clone();
        tokio::spawn(async move {
            if let Ok(list) = api.list_dms().await {
                let _ = tx.send(crate::app::ClientEvent::DmListLoaded {
                    items: list.conversations.clone(),
                });
                if let Some(c) = list
                    .conversations
                    .iter()
                    .find(|c| c.peer_handle == peer)
                {
                    if let Ok(resp) = api.get_dm(&c.id).await {
                        let _ = tx.send(crate::app::ClientEvent::DmThreadLoaded {
                            request_key,
                            conversation_id: resp.conversation.id,
                            peer_handle: resp.conversation.peer_handle,
                            messages: resp.messages,
                        });
                        return;
                    }
                }
            }
            // No existing conversation — stay on empty new-chat UI.
        });
        Ok(())
    }

    fn note_dm_opened(&mut self, conversation_id: &str) {
        if let Some(list) = self.view_cache.dm_list() {
            if let Some(c) = list.iter().find(|c| c.id == conversation_id) {
                self.unread_dms = self.unread_dms.saturating_sub(c.unread);
            }
        }
        self.view_cache.clear_dm_unread(conversation_id);
    }

    pub async fn open_dm_thread(&mut self, id: &str) -> Result<()> {
        self.pending_dm = Some(id.to_string());
        self.note_dm_opened(id);
        let mut draft = TextArea::default();
        draft.set_placeholder_text("Write a message…");

        if let Some(th) = self.view_cache.dm_thread(id) {
            let n = th.messages.len();
            self.screen = Screen::DmThread {
                state: DmThreadState {
                    conversation_id: th.conversation_id.clone(),
                    peer_handle: th.peer_handle.clone(),
                    messages: th.messages.clone(),
                    draft,
                    error: None,
                    scroll: n.saturating_sub(1),
                },
            };
            self.status = format!("@{} · chat", th.peer_handle);
            self.spawn_load_dm_thread(id.to_string(), id.to_string());
            return Ok(());
        }

        // Instant shell from list metadata if we have it.
        let peer = self
            .view_cache
            .dm_list()
            .and_then(|list| {
                list.iter()
                    .find(|c| c.id == id)
                    .map(|c| c.peer_handle.clone())
            })
            .unwrap_or_else(|| "…".into());
        self.screen = Screen::DmThread {
            state: DmThreadState {
                conversation_id: id.to_string(),
                peer_handle: peer,
                messages: Vec::new(),
                draft,
                error: None,
                scroll: 0,
            },
        };
        self.status = "loading chat…".into();
        self.spawn_load_dm_thread(id.to_string(), id.to_string());
        Ok(())
    }

    async fn handle_dm_list_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.go_home();
                self.status = "timeline".into();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Screen::DmList { state } = &mut self.screen {
                    if !state.items.is_empty() {
                        state.selected = (state.selected + 1).min(state.items.len() - 1);
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Screen::DmList { state } = &mut self.screen {
                    state.selected = state.selected.saturating_sub(1);
                }
            }
            KeyCode::Char('n') => {
                self.screen = Screen::DmNew {
                    state: DmNewState {
                        handle: String::new(),
                        error: None,
                    },
                };
                self.status = "who to message?".into();
            }
            KeyCode::Char('u') => {
                if let Err(e) = self.open_dm_list().await {
                    self.status = format!("refresh: {e:#}");
                }
            }
            KeyCode::Enter => {
                let id = if let Screen::DmList { state } = &self.screen {
                    state.items.get(state.selected).map(|c| c.id.clone())
                } else {
                    None
                };
                if let Some(id) = id {
                    if let Err(e) = self.open_dm_thread(&id).await {
                        self.status = format!("dm: {e:#}");
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_dm_new_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                let _ = self.open_dm_list().await;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Screen::DmNew { state } = &mut self.screen {
                    if state.handle.len() < 32
                        && (c.is_ascii_alphanumeric() || c == '_' || c == '@')
                    {
                        if c != '@' {
                            state.handle.push(c.to_ascii_lowercase());
                        }
                        state.error = None;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Screen::DmNew { state } = &mut self.screen {
                    state.handle.pop();
                }
            }
            KeyCode::Enter => {
                let handle = if let Screen::DmNew { state } = &self.screen {
                    state.handle.trim().trim_start_matches('@').to_string()
                } else {
                    return Ok(());
                };
                if handle.len() < 3 {
                    if let Screen::DmNew { state } = &mut self.screen {
                        state.error = Some("handle too short".into());
                    }
                    return Ok(());
                }
                // Verify user exists
                match self.api.get_profile(&handle).await {
                    Ok(p) => {
                        if p.is_me {
                            if let Screen::DmNew { state } = &mut self.screen {
                                state.error = Some("can't message yourself".into());
                            }
                            return Ok(());
                        }
                        if let Err(e) = self.open_dm_with_peer(&p.handle).await {
                            if let Screen::DmNew { state } = &mut self.screen {
                                state.error = Some(format!("{e:#}"));
                            }
                        }
                    }
                    Err(e) => {
                        if let Screen::DmNew { state } = &mut self.screen {
                            state.error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_dm_thread_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc {
            let _ = self.open_dm_list().await;
            return Ok(());
        }

        // Send: Enter (without shift) or Ctrl+S
        let send = matches!(key.code, KeyCode::Enter)
            || (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL));

        if send {
            let (conv, peer, content) = if let Screen::DmThread { state } = &self.screen {
                (
                    state.conversation_id.clone(),
                    state.peer_handle.clone(),
                    state.draft.lines().join("\n"),
                )
            } else {
                return Ok(());
            };
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() {
                if let Screen::DmThread { state } = &mut self.screen {
                    state.error = Some("empty message".into());
                }
                return Ok(());
            }
            if trimmed.chars().count() > POST_MAX_LEN {
                if let Screen::DmThread { state } = &mut self.screen {
                    state.error = Some(format!("max {POST_MAX_LEN} characters"));
                }
                return Ok(());
            }

            let result = if conv.is_empty() {
                self.api.send_dm_to(&peer, &trimmed).await
            } else {
                self.api.send_dm_in(&conv, &trimmed).await
            };

            match result {
                Ok(msg) => {
                    // Always reload so conversation_id is set and order is correct.
                    if let Err(e) = self.open_dm_thread(&msg.conversation_id).await {
                        // Fallback: append locally
                        if let Screen::DmThread { state } = &mut self.screen {
                            state.conversation_id = msg.conversation_id.clone();
                            state.messages.push(msg);
                            state.draft = {
                                let mut t = TextArea::default();
                                t.set_placeholder_text("Write a message…");
                                t
                            };
                            state.error = None;
                            state.scroll = state.messages.len().saturating_sub(1);
                        }
                        self.status = format!("sent (refresh failed: {e:#})");
                    } else {
                        self.status = "sent".into();
                    }
                }
                Err(e) => {
                    if let Screen::DmThread { state } = &mut self.screen {
                        state.error = Some(format!("{e:#}"));
                    }
                    self.status = format!("send failed: {e:#}");
                }
            }
            return Ok(());
        }

        // Scroll history with PgUp/PgDn / Ctrl+j/k
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Screen::DmThread { state } = &mut self.screen {
                        if !state.messages.is_empty() {
                            state.scroll =
                                (state.scroll + 1).min(state.messages.len().saturating_sub(1));
                        }
                    }
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Screen::DmThread { state } = &mut self.screen {
                        state.scroll = state.scroll.saturating_sub(1);
                    }
                    return Ok(());
                }
                _ => {}
            }
        }

        if let Screen::DmThread { state } = &mut self.screen {
            state.error = None;
            // Multiline: Shift+Enter inserts newline via textarea; plain keys go to draft
            if key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL) {
                // already handled
            } else {
                state.draft.input(key);
            }
        }
        Ok(())
    }

    async fn handle_edit_bio_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                let handle = self
                    .session
                    .as_ref()
                    .map(|s| s.handle.clone())
                    .unwrap_or_default();
                let _ = self.open_profile(&handle).await;
            }
            KeyCode::Enter => {
                let bio = if let Screen::EditBio { input, .. } = &self.screen {
                    input.clone()
                } else {
                    return Ok(());
                };
                let bio_opt = if bio.trim().is_empty() {
                    None
                } else {
                    Some(bio.trim())
                };
                match self.api.update_bio(bio_opt).await {
                    Ok(p) => {
                        self.status = "bio updated".into();
                        let _ = self.open_profile(&p.handle).await;
                    }
                    Err(e) => {
                        if let Screen::EditBio { error, .. } = &mut self.screen {
                            *error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Screen::EditBio { input, error, .. } = &mut self.screen {
                    if input.chars().count() < 160 {
                        input.push(c);
                    }
                    *error = None;
                }
            }
            KeyCode::Backspace => {
                if let Screen::EditBio { input, .. } = &mut self.screen {
                    input.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_block_user_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                let handle = if let Screen::BlockUser { handle, .. } = &self.screen {
                    handle.clone()
                } else {
                    return Ok(());
                };
                let _ = self.open_profile(&handle).await;
            }
            KeyCode::Enter => {
                let (handle, reason) = if let Screen::BlockUser { handle, reason, .. } =
                    &self.screen
                {
                    (handle.clone(), reason.clone())
                } else {
                    return Ok(());
                };
                let trimmed = reason.trim();
                if trimmed.is_empty() {
                    if let Screen::BlockUser { error, .. } = &mut self.screen {
                        *error = Some("reason required — say why you're blocking".into());
                    }
                    return Ok(());
                }
                if trimmed.chars().count() > BLOCK_REASON_MAX_LEN {
                    if let Screen::BlockUser { error, .. } = &mut self.screen {
                        *error = Some(format!("max {BLOCK_REASON_MAX_LEN} characters"));
                    }
                    return Ok(());
                }
                match self.api.block(&handle, trimmed).await {
                    Ok(_) => {
                        self.blocked_handles.insert(handle.clone());
                        self.following_handles.remove(&handle);
                        self.go_home();
                        self.status = format!("blocked @{handle}");
                        let _ = self.refresh_feed().await;
                    }
                    Err(e) => {
                        if let Screen::BlockUser { error, .. } = &mut self.screen {
                            *error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Screen::BlockUser { reason, error, .. } = &mut self.screen {
                    if reason.chars().count() < BLOCK_REASON_MAX_LEN {
                        reason.push(c);
                    }
                    *error = None;
                }
            }
            KeyCode::Backspace => {
                if let Screen::BlockUser { reason, .. } = &mut self.screen {
                    reason.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn open_edit_post(&mut self, post_id: &str, content: &str, return_to: ReturnTo) {
        let mut ta = TextArea::default();
        ta.insert_str(content);
        self.screen = Screen::Compose {
            textarea: ta,
            error: None,
            parent_id: None,
            edit_id: Some(post_id.to_string()),
            return_to,
            color_ac_idx: 0,
        };
    }
}

fn flatten_search(resp: SearchResponse) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for u in resp.users {
        hits.push(SearchHit::User(u));
    }
    for t in resp.tags {
        hits.push(SearchHit::Tag(t.name, t.post_count));
    }
    for p in resp.posts {
        hits.push(SearchHit::Post(p));
    }
    hits
}
